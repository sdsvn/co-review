//! Threads in the editor (inline-review): each thread of the active review that is on lines of a file shows under
//! its last line, between the editor's lines; a gutter column has a marker per thread and a **+** to start one (drag
//! for several lines); folded threads are only their gutter marker.
//!
//! **How a thread sits between lines.** Floem's editor has no widgets between lines, but its text layouts take
//! multi-line phantom text (as Lapce's multi-line error lens does). Under a line with threads, the document's
//! phantom text gets as many blank rows as the threads are high ([`phantom_text`], called from `Doc::phantom_text`),
//! so the editor lays out real space there; [`editor_threads`], an overlay over the editor's text, puts the thread
//! views on those rows, following scrolling and edits. Each thread's measured height sets its rows.
//!
//! **Live placement.** In an open editor threads follow the text as it is typed, before it is saved: every edit
//! (`Doc::apply_deltas` calls [`doc_edited`]) carries each thread's tracked range through the edit with
//! `review_anchor::track`, which keeps it on its code (`active` / `modified`) or takes it off the code (`removed`,
//! `ambiguous`: out of scope, never shown on code). Placing (the first placement's diff against a snapshot, and
//! each edit's) runs on a background thread on a snapshot of the text: one at a time per file, the next one starting
//! from the text the last one placed in. Files without threads do nothing on an edit. Saving a file with threads
//! resolves their stored anchors ([`doc_saved`]), as the spec's "a commented file changes".

use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use floem::{
    IntoView, View,
    action::exec_after,
    event::{Event, EventListener},
    kurbo::{Point, Rect},
    reactive::{
        Memo, RwSignal, Scope, SignalGet, SignalUpdate, SignalWith, create_memo,
        create_rw_signal,
    },
    style::CursorStyle,
    views::{
        Decorators, container, dyn_container, dyn_stack, editor::phantom_text::{
            PhantomText, PhantomTextKind,
        }, empty, h_stack, label, svg, tooltip,
    },
};
use lapce_core::cursor::{CursorAffinity, CursorMode};
use lapce_core::buffer::rope_text::RopeText;
use lapce_xi_rope::Rope;
use review_anchor::{AnchorStatus, TrackedRange};
use review_model::{
    CodeAnchor, CodeLocation, LocationKind, Position, Range, Review, ReviewThread,
    ThreadIntent, ThreadStatus,
};

use super::{
    ReviewCore, drafts,
    thread::{
        ThreadViewOptions, acp_stand_in, agent_link, ask_label, ask_tooltip,
        location_label, open_location, reply_box, text_button, thread_view,
    },
};
use crate::{
    config::{color::LapceColor, icon::LapceIcons},
    doc::Doc,
    editor::EditorData,
    id::WindowTabId,
    window_tab::WindowTabData,
};

/// Rows reserved for a thread not measured yet.
const ESTIMATED_ROWS: usize = 6;
/// Space between a thread and the code around it, in pixels.
const GAP: f64 = 8.0;

/// What shows at a place in a file: a thread or a draft.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ItemKey {
    Thread { review_id: String, thread_id: String },
    Draft(String),
}

/// A thread or draft placed in an open file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Item {
    pub key: ItemKey,
    /// The 0-based line it shows under (the last line of its range).
    pub line: usize,
    /// Only the gutter marker shows.
    pub folded: bool,
    pub kind: MarkerKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MarkerKind {
    Comment,
    Question,
    Proposed,
    Resolved,
    Draft,
}

/// What to place in a file, before placement: a thread or draft and where it was put.
#[derive(Clone)]
struct Entry {
    key: ItemKey,
    location: CodeLocation,
    folded: bool,
    kind: MarkerKind,
    summary: String,
}

/// The tracked range of a thread or draft in the text last placed in ([`Tracking::text`]), and the stored target it
/// came from.
#[derive(Clone, Debug, PartialEq)]
struct Live {
    basis: String,
    tracked: Option<TrackedRange>,
    /// The line it shows under; `None` when out of scope.
    line: Option<usize>,
}

/// What a background placement starts from.
enum Start {
    /// A live range in the text last placed in.
    Tracked(Box<TrackedRange>),
    /// The stored location: first placement.
    Location(Box<CodeLocation>),
}

struct Job {
    key: ItemKey,
    basis: String,
    start: Start,
}

/// The background placement of one file's threads.
#[derive(Default)]
struct Tracking {
    /// The text the live ranges are in (a rope: cloning it is cheap).
    text: Option<Rope>,
    /// The editor's buffer, read when a placement starts.
    buffer: Option<RwSignal<lapce_core::buffer::Buffer>>,
    /// The review's anchor snapshots, for first placements.
    snapshots: Option<PathBuf>,
    running: bool,
    /// Edited or refreshed while a placement ran: place again when it ends.
    again: bool,
}

/// The threads and drafts of one open file (shared by every editor on it).
pub struct DocThreads {
    uri: String,
    scope: Scope,
    entries: RefCell<Vec<Entry>>,
    live: RefCell<HashMap<ItemKey, Live>>,
    tracking: RefCell<Tracking>,
    pub items: RwSignal<Vec<Item>>,
    /// Each item's first message (or the draft's text), for its gutter marker's hover. Apart from `items`, so typing
    /// in a draft changes no item: the editor isn't laid out again and the draft stays where it is.
    pub summaries: RwSignal<HashMap<ItemKey, String>>,
    /// Phantom rows of each unfolded item, from its measured height.
    pub rows: RwSignal<HashMap<ItemKey, usize>>,
}

thread_local! {
    static DOCS: RefCell<HashMap<PathBuf, Rc<DocThreads>>> = RefCell::new(HashMap::new());
    static FOLDED: RefCell<HashMap<WindowTabId, RwSignal<HashMap<String, bool>>>> =
        RefCell::new(HashMap::new());
    static FOCUS: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn doc_path(doc: &Doc) -> Option<PathBuf> {
    doc.content.with_untracked(|content| content.path().cloned())
}

fn doc_threads(path: &Path) -> Option<Rc<DocThreads>> {
    DOCS.with(|docs| docs.borrow().get(path).cloned())
}

fn doc_threads_or_new(path: &Path) -> Rc<DocThreads> {
    DOCS.with(|docs| {
        docs.borrow_mut()
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                let cx = Scope::new();
                Rc::new(DocThreads {
                    uri: review_store::path_to_file_uri(
                        &review_store::real_path(path).to_string_lossy(),
                    ),
                    scope: cx,
                    entries: RefCell::default(),
                    live: RefCell::default(),
                    tracking: RefCell::default(),
                    items: cx.create_rw_signal(Vec::new()),
                    summaries: cx.create_rw_signal(HashMap::new()),
                    rows: cx.create_rw_signal(HashMap::new()),
                })
            })
            .clone()
    })
}

/// Threads folded or unfolded by the reviewer in a window tab (thread id → folded); others use their default.
fn folded_signal(window_tab: &WindowTabData) -> RwSignal<HashMap<String, bool>> {
    FOLDED.with(|folded| {
        *folded
            .borrow_mut()
            .entry(window_tab.window_tab_id)
            .or_insert_with(|| Scope::new().create_rw_signal(HashMap::new()))
    })
}

/// Folded unless the reviewer opened it: resolved threads, and proposed findings on a pull request.
fn folded_by_default(review: &Review, thread: &ReviewThread) -> bool {
    thread.status == ThreadStatus::Resolved
        || (thread.status == ThreadStatus::Proposed && review.bundle.is_some())
}

// ---- Hooks called from Lapce's `Doc` ----

/// The blank rows under `line` for the threads shown there (`co-review:` hook in `Doc::phantom_text`). `line_len` is
/// the line's length with its line ending, where the rows start.
pub fn phantom_text(doc: &Doc, line: usize, line_len: usize) -> Option<PhantomText> {
    let threads = doc_threads(&doc_path(doc)?)?;
    let rows = threads.rows_at(line);
    if rows == 0 {
        return None;
    }
    Some(PhantomText {
        kind: PhantomTextKind::Diagnostic,
        col: line_len,
        affinity: Some(CursorAffinity::Backward),
        // A row needs a glyph to count as a visual line; a no-break space is one that draws nothing.
        text: "\n\u{a0}".repeat(rows),
        font_size: None,
        fg: None,
        bg: None,
        under_line: None,
    })
}

/// The document changed (`co-review:` hook in `Doc::apply_deltas`): threads follow the edit before it is saved, placed
/// in the background from the text they were last placed in (so `_before`, the text before this edit, isn't needed).
/// A file without threads does nothing.
pub fn doc_edited(doc: &Doc, _before: &Rope) {
    let Some(threads) = doc_path(doc).and_then(|path| doc_threads(&path)) else {
        return;
    };
    if threads.entries.borrow().is_empty() {
        return;
    }
    threads.sync(Some(doc.buffer));
}

/// A file was saved (`co-review:` hook in `Doc::save`): the stored anchors of the reviews with threads in it are
/// resolved against it, off the UI thread.
pub fn doc_saved(path: &Path) {
    let Some(threads) = doc_threads(path) else {
        return;
    };
    let mut review_ids: Vec<String> = threads
        .entries
        .borrow()
        .iter()
        .filter_map(|entry| match &entry.key {
            ItemKey::Thread { review_id, .. } => Some(review_id.clone()),
            ItemKey::Draft(_) => None,
        })
        .collect();
    review_ids.dedup();
    let Some(core) = ReviewCore::get().filter(|_| !review_ids.is_empty()) else {
        return;
    };
    let bundles = std::sync::Arc::clone(&core.services.bundles);
    let resolving = std::thread::Builder::new()
        .name("ReviewResolveAnchors".to_owned())
        .spawn(move || {
            for review_id in review_ids {
                if let Err(error) = bundles.resolve_anchors(&review_id) {
                    tracing::error!("resolving the anchors of review {review_id} after a save: {error:#}");
                }
            }
        });
    if let Err(error) = resolving {
        tracing::error!("anchors not resolved after a save: {error}");
    }
}

/// Places `jobs` in `after` (on a background thread): tracked ranges are carried from `before` through the edits
/// between the two texts, stored locations placed for the first time (through the snapshot of their version in
/// `snapshots`).
fn placements(before: Option<&str>, after: &str, jobs: Vec<Job>, snapshots: Option<&Path>) -> Vec<(ItemKey, Live)> {
    let tracks = jobs.iter().any(|job| matches!(job.start, Start::Tracked(_)));
    let edits = before.filter(|_| tracks).map(|before| review_anchor::edits_between(before, after));
    jobs.into_iter()
        .map(|job| {
            let (tracked, fallback_line) = match job.start {
                Start::Tracked(tracked) => (
                    Some(match &edits {
                        Some(edits) => review_anchor::track(&tracked, before, after, "live", Some(edits)),
                        None => review_anchor::track(&tracked, None, after, "live", None),
                    }),
                    None,
                ),
                Start::Location(location) => (
                    live_range(&location, after, snapshots),
                    location.range.as_ref().map(end_line),
                ),
            };
            let line = match &tracked {
                Some(tracked) => match tracked.status {
                    AnchorStatus::Removed | AnchorStatus::Ambiguous => None,
                    AnchorStatus::Active | AnchorStatus::Modified => {
                        let end = if tracked.to > tracked.from { tracked.to - 1 } else { tracked.from };
                        Some(review_anchor::position_at(after, end as usize).0)
                    }
                },
                None => fallback_line,
            };
            (
                job.key,
                Live {
                    basis: job.basis,
                    tracked,
                    line,
                },
            )
        })
        .collect()
}

impl DocThreads {
    fn rows_at(&self, line: usize) -> usize {
        let rows = self.rows;
        self.items.with_untracked(|items| {
            rows.with_untracked(|rows| {
                items
                    .iter()
                    .filter(|item| item.line == line && !item.folded)
                    .map(|item| rows.get(&item.key).copied().unwrap_or(ESTIMATED_ROWS))
                    .sum()
            })
        })
    }

    /// Rows of the unfolded items before `key` on its line.
    fn rows_before(&self, key: &ItemKey, line: usize) -> usize {
        let rows = self.rows;
        self.items.with(|items| {
            rows.with(|rows| {
                items
                    .iter()
                    .filter(|item| item.line == line && !item.folded)
                    .take_while(|item| &item.key != key)
                    .map(|item| rows.get(&item.key).copied().unwrap_or(ESTIMATED_ROWS))
                    .sum()
            })
        })
    }

    /// Takes the window tab's active review and drafts for this file and places them in the editor's text.
    fn refresh(
        self: &Rc<Self>,
        doc: &Doc,
        review: Option<&Review>,
        drafts: &[super::workspace::ReviewDraft],
        folded: &HashMap<String, bool>,
    ) {
        if !doc.loaded.get_untracked() {
            // Placed once the text is there; nothing is tracked against an empty buffer.
            self.live.borrow_mut().clear();
            self.tracking.borrow_mut().text = None;
            return;
        }
        let mut entries = Vec::new();
        if let Some(review) = review {
            for thread in &review.threads {
                if !self.shows(&thread.location) {
                    continue;
                }
                let kind = match thread.status {
                    ThreadStatus::Resolved => MarkerKind::Resolved,
                    ThreadStatus::Proposed => MarkerKind::Proposed,
                    ThreadStatus::Open if thread.intent == ThreadIntent::Question => MarkerKind::Question,
                    ThreadStatus::Open => MarkerKind::Comment,
                };
                entries.push(Entry {
                    key: ItemKey::Thread {
                        review_id: review.id.clone(),
                        thread_id: thread.id.clone(),
                    },
                    location: thread.location.clone(),
                    folded: folded
                        .get(&thread.id)
                        .copied()
                        .unwrap_or_else(|| folded_by_default(review, thread)),
                    kind,
                    summary: thread
                        .messages
                        .first()
                        .map(|message| summary(&message.body))
                        .unwrap_or_default(),
                });
            }
            for draft in drafts {
                if draft.review_id != review.id || !self.shows(&draft.location) {
                    continue;
                }
                entries.push(Entry {
                    key: ItemKey::Draft(draft.id.clone()),
                    location: draft.location.clone(),
                    folded: false,
                    kind: MarkerKind::Draft,
                    summary: if draft.body.is_empty() {
                        "Draft".to_string()
                    } else {
                        summary(&draft.body)
                    },
                });
            }
        }
        self.tracking.borrow_mut().snapshots = review.and_then(|review| {
            ReviewCore::get().map(|core| {
                review_store::snapshots::snapshots_dir(&core.services.store.workspace_dir(&review.workspace_root))
            })
        });
        self.live
            .borrow_mut()
            .retain(|key, _| entries.iter().any(|entry| &entry.key == key));
        tracing::debug!(
            "{} review threads and drafts for {} (review {:?})",
            entries.len(),
            self.uri,
            review.map(|review| review.id.as_str())
        );
        *self.entries.borrow_mut() = entries;
        self.place();
        self.sync(Some(doc.buffer));
    }

    /// Places the entries in the buffer's current text on a background thread, unless one is placing already (then
    /// again when it ends). Entries placed before follow the edits since; new ones (or with a new stored target) are
    /// placed for the first time.
    fn sync(self: &Rc<Self>, buffer: Option<RwSignal<lapce_core::buffer::Buffer>>) {
        let mut tracking = self.tracking.borrow_mut();
        if let Some(buffer) = buffer {
            tracking.buffer = Some(buffer);
        }
        if tracking.running {
            tracking.again = true;
            return;
        }
        let Some(buffer) = tracking.buffer else {
            return;
        };
        let after = buffer.with_untracked(|buffer| buffer.text().clone());
        let before = tracking.text.clone().filter(|before| !before.ptr_eq(&after));
        let unchanged = tracking.text.as_ref().is_some_and(|before| before.ptr_eq(&after));
        let jobs: Vec<Job> = {
            let live = self.live.borrow();
            self.entries
                .borrow()
                .iter()
                .filter_map(|entry| {
                    let basis = basis(&entry.location);
                    let start = match live.get(&entry.key) {
                        Some(current) if current.basis == basis => match (&current.tracked, &before) {
                            (Some(tracked), Some(_)) => Start::Tracked(Box::new(tracked.clone())),
                            // In the text already, or on a line that doesn't follow edits.
                            _ if unchanged || current.tracked.is_none() => return None,
                            _ => Start::Location(Box::new(entry.location.clone())),
                        },
                        _ => Start::Location(Box::new(entry.location.clone())),
                    };
                    Some(Job {
                        key: entry.key.clone(),
                        basis,
                        start,
                    })
                })
                .collect()
        };
        if jobs.is_empty() {
            tracking.text = Some(after);
            return;
        }
        tracking.running = true;
        let snapshots = tracking.snapshots.clone();
        drop(tracking);
        let this = Rc::clone(self);
        let done = floem::ext_event::create_ext_action(self.scope, move |(after, placed): (Rope, Vec<(ItemKey, Live)>)| {
            let again = {
                let mut tracking = this.tracking.borrow_mut();
                tracking.running = false;
                tracking.text = Some(after);
                std::mem::take(&mut tracking.again)
            };
            {
                let entries = this.entries.borrow();
                let mut live = this.live.borrow_mut();
                for (key, placed) in placed {
                    // Kept only while the entry is still there with the same stored target.
                    if entries
                        .iter()
                        .any(|entry| entry.key == key && basis(&entry.location) == placed.basis)
                    {
                        live.insert(key, placed);
                    }
                }
            }
            this.place();
            if again {
                this.sync(None);
            }
        });
        let placing = std::thread::Builder::new()
            .name("ReviewThreadPlacement".to_owned())
            .spawn(move || {
                let before = before.map(|before| before.to_string());
                let after_text = after.to_string();
                let placed = placements(before.as_deref(), &after_text, jobs, snapshots.as_deref());
                done((after, placed));
            });
        if let Err(error) = placing {
            tracing::error!("review threads not placed: {error}");
            self.tracking.borrow_mut().running = false;
        }
    }

    /// Whether a location shows in this file: lines of it.
    fn shows(&self, location: &CodeLocation) -> bool {
        matches!(
            location.kind,
            LocationKind::Line | LocationKind::Range | LocationKind::Symbol
        ) && location.range.is_some()
            && location
                .uri
                .as_deref()
                .is_some_and(|uri| uri == self.uri || real_uri(uri) == self.uri)
    }

    /// Places the entries on their live lines; out-of-scope ones (and ones not placed yet) are left out. The editors
    /// on the file lay out again when the items change (see `doc_overlay`), not when only a summary does.
    fn place(&self) {
        let (items, summaries) = placed_items(&self.entries.borrow(), &self.live.borrow());
        if self.items.with_untracked(|current| *current != items) {
            tracing::debug!(
                "review threads in {}: {:?}",
                self.uri,
                items.iter().map(|item| (item.line, item.folded)).collect::<Vec<_>>()
            );
            self.items.set(items);
        }
        if self.summaries.with_untracked(|current| *current != summaries) {
            self.summaries.set(summaries);
        }
    }

    /// The measured height of an item: its rows change (and the editor lays out again) when it needs other rows.
    fn measured(&self, doc: &Doc, key: &ItemKey, height: f64, line_height: f64) {
        if height <= 0.0 || line_height <= 0.0 {
            return;
        }
        let rows = ((height + GAP) / line_height).ceil() as usize;
        let changed = self
            .rows
            .with_untracked(|current| current.get(key) != Some(&rows));
        if changed {
            tracing::debug!("review thread {key:?} is {height}px high: {rows} rows");
            // Together, so the views place themselves once, on the layout with the new rows.
            floem::reactive::batch(|| {
                self.rows.update(|current| {
                    current.insert(key.clone(), rows);
                });
                doc.clear_text_cache();
            });
        }
    }
}

/// A `file:` URI resolved through symbolic links, as the store keys files.
fn real_uri(uri: &str) -> String {
    match review_store::file_uri_to_path(uri) {
        Some(path) => file_uri(Path::new(&path)),
        None => uri.to_string(),
    }
}

/// The items of a file (on their live lines, in line order) and their summaries. An item depends only on where an
/// entry shows and how, never on its text.
fn placed_items(entries: &[Entry], live: &HashMap<ItemKey, Live>) -> (Vec<Item>, HashMap<ItemKey, String>) {
    let mut items: Vec<Item> = entries
        .iter()
        .filter_map(|entry| {
            let line = live.get(&entry.key)?.line?;
            Some(Item {
                key: entry.key.clone(),
                line,
                folded: entry.folded,
                kind: entry.kind,
            })
        })
        .collect();
    items.sort_by_key(|item| item.line);
    let summaries = entries
        .iter()
        .map(|entry| (entry.key.clone(), entry.summary.clone()))
        .collect();
    (items, summaries)
}

fn summary(body: &str) -> String {
    let line = body.lines().find(|line| !line.trim().is_empty()).unwrap_or("");
    let mut text: String = line.chars().take(120).collect();
    if line.chars().count() > 120 {
        text.push('…');
    }
    text
}

/// What a live range was computed from: the stored tracked range (or the range, without one).
fn basis(location: &CodeLocation) -> String {
    match &location.tracked {
        Some(tracked) => format!(
            "{}:{}:{}:{:?}",
            tracked.version, tracked.from, tracked.to, tracked.status
        ),
        None => format!("{:?}", location.range),
    }
}

/// The last line of a range (a range ending at the start of a line ends on the line before).
fn end_line(range: &Range) -> usize {
    if range.end.character == 0 && range.end.line > range.start.line {
        range.end.line as usize - 1
    } else {
        range.end.line as usize
    }
}

/// The location's target in the editor's `text`: its stored tracked range carried into the text (through the
/// snapshot of the version it was made on), or its range when it has none.
fn live_range(location: &CodeLocation, text: &str, snapshots: Option<&Path>) -> Option<TrackedRange> {
    if let Some(tracked) = &location.tracked {
        let version = review_model::content_version(text);
        if tracked.version == version {
            return Some(tracked.clone());
        }
        let before = snapshots
            .and_then(|dir| std::fs::read_to_string(dir.join(&tracked.version)).ok());
        return Some(review_anchor::track(tracked, before.as_deref(), text, &version, None));
    }
    let range = location.range.as_ref()?;
    let from = review_anchor::offset_at(text, range.start.line as usize, range.start.character as usize);
    let to = review_anchor::offset_at(text, range.end.line as usize, range.end.character as usize);
    (to > from).then(|| review_anchor::range_in(text, "live", from, to, AnchorStatus::Active, None))
}

// ---- The overlay with the threads, over an editor's text ----

/// What places an item's view on its rows.
struct ItemRows<'a> {
    /// Phantom rows under the item's line, for every item there.
    rows_total: usize,
    /// Rows of the items before this one on the line.
    rows_before: usize,
    /// Visual lines of the line's text layout, phantom rows included.
    layout_lines: usize,
    /// The line's visual lines on screen: (index in the line, y).
    on_screen: &'a [(usize, f64)],
    /// The screen's last visual line is one of the line's.
    line_ends_screen: bool,
    line_height: f64,
}

#[derive(Debug, PartialEq)]
enum Placed {
    /// The y of the item's first row.
    At(f64),
    OffScreen,
    /// The layout or the screen lines don't have the item's rows yet (the editor is being laid out again).
    Unsettled,
}

/// Where an item's view goes: on its first row, when its rows are on screen.
fn item_top(rows: ItemRows) -> Placed {
    let Some(first_row) = rows.layout_lines.checked_sub(rows.rows_total) else {
        return Placed::Unsettled;
    };
    let found = rows
        .on_screen
        .iter()
        .find(|(index, _)| *index >= first_row)
        .map(|(index, y)| y - (index - first_row) as f64 * rows.line_height);
    match found {
        Some(y) => Placed::At(y + rows.rows_before as f64 * rows.line_height),
        // The line's text is on screen and the screen goes on after it, but its rows aren't there.
        None if !rows.on_screen.is_empty() && !rows.line_ends_screen => Placed::Unsettled,
        None => Placed::OffScreen,
    }
}

/// The threads and drafts of the editor's file, on the rows reserved for them (`co-review:` hook in
/// `editor_container_view`). Absolutely positioned over the editor; only the threads take pointer events.
pub fn editor_threads(window_tab: Rc<WindowTabData>, e_data: RwSignal<EditorData>) -> impl View {
    let doc_signal = e_data.with_untracked(|editor| editor.doc_signal());
    let origin = create_rw_signal(Point::ZERO);
    let size = create_rw_signal(Rect::ZERO);
    let bar = super::selection_bar::selection_bar(window_tab.clone(), e_data, origin, size);
    floem::views::stack((
        dyn_container(
            move || doc_signal.with(|doc| doc.content.with(|content| content.path().cloned())),
            move |path| match path {
                Some(path) => doc_overlay(window_tab.clone(), e_data, path, origin, size).into_any(),
                None => empty().into_any(),
            },
        )
        .style(|s| s.absolute().size_full()),
        bar,
    ))
    .on_move(move |point| origin.set(point))
    .on_resize(move |rect| size.set(rect))
    // The overlay covers the editor and its gutter: it must let scrolling and clicks through to them. Floem
    // inherits pointer-events, so the threads on it turn them back on.
    .style(|s| s.absolute().size_full().pointer_events_none())
    .debug_name("Review Threads")
}

fn doc_overlay(
    window_tab: Rc<WindowTabData>,
    e_data: RwSignal<EditorData>,
    path: PathBuf,
    origin: RwSignal<Point>,
    size: RwSignal<Rect>,
) -> impl View {
    let threads = doc_threads_or_new(&path);
    let (editor, doc_signal, config, content_origin) = e_data.with_untracked(|editor| {
        (
            editor.editor.clone(),
            editor.doc_signal(),
            editor.common.config,
            editor.window_origin(),
        )
    });
    drafts::keep(&window_tab);
    let folded = folded_signal(&window_tab);
    {
        let threads = threads.clone();
        let review = window_tab.review.clone();
        Scope::current().create_effect(move |_| {
            let doc = doc_signal.get();
            doc.loaded.get();
            let active = review.active_review();
            let drafts = review.drafts.get();
            let folded = folded.get();
            let drafts: Vec<_> = drafts.into_iter().collect();
            threads.refresh(&doc, active.as_deref(), &drafts, &folded);
        });
    }
    let viewport = editor.viewport;
    let screen_lines = editor.screen_lines;
    let items = threads.items;
    // Placed (in the background): the editor lays out the rows under the new lines.
    Scope::current().create_effect(move |_| {
        items.with(|_| ());
        doc_signal.get_untracked().clear_text_cache();
    });
    let shown = create_memo(move |_| {
        items.with(|items| {
            items
                .iter()
                .filter(|item| !item.folded)
                .map(|item| item.key.clone())
                .collect::<Vec<_>>()
        })
    });
    dyn_stack(
        move || shown.get(),
        |key| key.clone(),
        move |key| {
            let threads = threads.clone();
            let editor = editor.clone();
            let window_tab = window_tab.clone();
            let content = match &key {
                ItemKey::Thread { review_id, thread_id } => {
                    let thread_id_for_hide = thread_id.clone();
                    thread_view(
                        window_tab.clone(),
                        review_id.clone(),
                        thread_id.clone(),
                        ThreadViewOptions {
                            inline: true,
                            on_hide: Some(Rc::new(move || {
                                folded.update(|folded| {
                                    folded.insert(thread_id_for_hide.clone(), true);
                                })
                            })),
                            on_open: None,
                        },
                    )
                    .into_any()
                }
                ItemKey::Draft(id) => draft_view(window_tab.clone(), id.clone()).into_any(),
            };
            let measured_key = key.clone();
            let measure_threads = threads.clone();
            let last_top = Rc::new(std::cell::Cell::new(None::<f64>));
            container(content)
                .on_resize(move |rect| {
                    let line_height = config.get_untracked().editor.line_height() as f64;
                    let doc = doc_signal.get_untracked();
                    measure_threads.measured(&doc, &measured_key, rect.height(), line_height);
                })
                .on_event_stop(EventListener::PointerDown, |_| {})
                .style(move |s| {
                    let s = s.pointer_events_auto();
                    // An overlay whose editor view is gone (the file's threads outlive it): nothing to place.
                    let (Some(origin), Some(size), Some(content_origin)) =
                        (origin.try_get(), size.try_get(), content_origin.try_get())
                    else {
                        return s.hide();
                    };
                    let config = config.get();
                    let line_height = config.editor.line_height() as f64;
                    let viewport = viewport.get();
                    let line = items.with(|items| {
                        items.iter().find(|item| item.key == key).map(|item| item.line)
                    });
                    let top = line.and_then(|line| {
                        // Without creating layouts' events: a layout made here would lay the editor out again
                        // from inside this style, re-entering it.
                        let layout_lines = editor.text_layout_trigger(line, false).line_count();
                        let rows: Vec<(usize, f64)> = screen_lines.with(|screen_lines| {
                            screen_lines
                                .iter_line_info()
                                .filter(|info| info.vline_info.rvline.line == line)
                                .map(|info| (info.vline_info.rvline.line_index, info.vline_y))
                                .collect()
                        });
                        let last_row_on_screen = screen_lines.with(|screen_lines| {
                            screen_lines.lines.last().is_some_and(|rvline| rvline.line == line)
                        });
                        let placed = item_top(ItemRows {
                            rows_total: threads.rows_at(line),
                            rows_before: threads.rows_before(&key, line),
                            layout_lines,
                            on_screen: &rows,
                            line_ends_screen: last_row_on_screen,
                            line_height,
                        });
                        match placed {
                            Placed::At(y) => {
                                let top = y - viewport.y0 + GAP / 2.0;
                                last_top.set(Some(top));
                                Some(top)
                            }
                            Placed::OffScreen => {
                                last_top.set(None);
                                None
                            }
                            // The editor is being laid out again: where it was.
                            Placed::Unsettled => last_top.get(),
                        }
                    });
                    let left = (content_origin.x - origin.x).max(0.0) + 8.0;
                    let width = (size.width() - left - 24.0).clamp(200.0, 960.0);
                    tracing::trace!("review thread {key:?} at line {line:?}: top {top:?}, left {left}, width {width}");
                    match top {
                        Some(top) => s
                            .absolute()
                            .margin_top(top as f32)
                            .margin_left(left as f32)
                            .width(width as f32),
                        // Off screen: laid out out of sight, so it can still be measured.
                        None => s
                            .absolute()
                            .margin_top(-10000.0_f32)
                            .margin_left(left as f32)
                            .width(width as f32),
                    }
                })
        },
    )
    .style(|s| s.absolute().size_full())
}

/// A draft in the editor: its text and **Comment** / **Ask Agent**, or discard.
pub fn draft_view(window_tab: Rc<WindowTabData>, draft_id: String) -> impl View {
    let config = window_tab.common.config;
    let drafts = window_tab.review.drafts;
    let draft = {
        let draft_id = draft_id.clone();
        create_memo(move |_| {
            drafts.with(|drafts| drafts.iter().find(|draft| draft.id == draft_id).cloned())
        })
    };
    let initial = draft.with_untracked(|draft| draft.as_ref().map(|d| d.body.clone()).unwrap_or_default());
    let asking = move || draft.with(|d| d.as_ref().is_some_and(|d| d.intent == ThreadIntent::Question));
    // The review's agent: its name, how it answers, and whether an ACP agent could answer here instead.
    let agent = {
        let review = window_tab.review.clone();
        let listening = ReviewCore::get().map(|core| core.listening);
        create_memo(move |_| {
            let review_id = draft.with(|d| d.as_ref().map(|d| d.review_id.clone()))?;
            let agent = review.reviews.with(|reviews| {
                reviews.iter().find(|review| review.id == review_id).and_then(|review| review.agent.clone())
            })?;
            let listening = listening.is_some_and(|listening| {
                listening.with(|listening| listening.get(&review_id).copied().unwrap_or(false))
            });
            Some((agent.name.clone(), agent_link(&agent, listening), acp_stand_in(&agent).is_some()))
        })
    };
    let title = label(move || {
        let place = draft
            .with(|d| d.as_ref().map(|d| location_label_of(&d.location)))
            .unwrap_or_default();
        if asking() {
            format!("Ask the agent · {place}")
        } else {
            format!("New comment · {place}")
        }
    })
    .style(|s| s.font_bold());
    let discard = {
        let window_tab = window_tab.clone();
        let draft_id = draft_id.clone();
        label(|| "×")
            .on_click_stop(move |_| drafts::discard(&window_tab, &draft_id))
            .style(move |s| {
                s.padding_horiz(6.0)
                    .cursor(CursorStyle::Pointer)
                    .selectable(false)
                    .color(config.get().color(LapceColor::EDITOR_DIM))
            })
    };
    let input = {
        let (submit_tab, change_tab) = (window_tab.clone(), window_tab.clone());
        let (submit_id, change_id) = (draft_id.clone(), draft_id.clone());
        reply_box(
            window_tab.clone(),
            "Leave a comment… (Enter to send)",
            initial,
            move |body| {
                drafts::set_body(&submit_tab, &submit_id, body);
                let intent = draft.with_untracked(|d| d.as_ref().map_or(ThreadIntent::Comment, |d| d.intent));
                drafts::submit(&submit_tab, &submit_id, intent);
            },
            move |body| drafts::set_body(&change_tab, &change_id, body),
        )
    };
    let input_id = input.id();
    let focus_wanted = FOCUS.with(|focus| focus.borrow().as_deref() == Some(draft_id.as_str()));
    if focus_wanted {
        FOCUS.with(|focus| *focus.borrow_mut() = None);
        exec_after(Duration::from_millis(50), move |_| input_id.request_focus());
    }
    let send = |intent: ThreadIntent, text: &'static str, primary: bool| {
        let window_tab = window_tab.clone();
        let draft_id = draft_id.clone();
        text_button(text.to_string(), primary, move || drafts::submit(&window_tab, &draft_id, intent), config)
    };
    let comment = send(ThreadIntent::Comment, "Comment", true);
    let ask = {
        let window_tab = window_tab.clone();
        let draft_id = draft_id.clone();
        dyn_container(
            move || agent.get(),
            move |agent| {
                let Some((name, link, can_answer_here)) = agent else {
                    return empty().into_any();
                };
                let (window_tab, draft_id) = (window_tab.clone(), draft_id.clone());
                let tip = ask_tooltip(&name, link, can_answer_here);
                crate::app::tooltip_label(
                    config,
                    text_button(
                        ask_label(&name, link),
                        false,
                        move || drafts::submit(&window_tab, &draft_id, ThreadIntent::Question),
                        config,
                    ),
                    move || tip.clone(),
                )
                .into_any()
            },
        )
    };
    floem::views::v_stack((
        h_stack((title, empty().style(|s| s.flex_grow(1.0_f32)), discard)).style(|s| s.width_full().items_center()),
        input.style(|s| s.margin_top(6.0)),
        h_stack((
            empty().style(|s| s.flex_grow(1.0_f32)),
            ask,
            comment,
        ))
        .style(|s| s.width_full().margin_top(6.0)),
    ))
    .style(move |s| {
        let config = config.get();
        s.width_full()
            .flex_col()
            .padding(8.0)
            .border(1.0)
            .border_radius(6.0)
            .border_color(config.color(if asking() { LapceColor::EDITOR_LINK } else { LapceColor::LAPCE_BORDER }))
            .background(config.color(LapceColor::PANEL_BACKGROUND))
            .color(config.color(LapceColor::EDITOR_FOREGROUND))
    })
    .debug_name("Review Draft")
}

fn location_label_of(location: &CodeLocation) -> String {
    let thread = ReviewThread {
        id: String::new(),
        number: 0,
        location: location.clone(),
        messages: Vec::new(),
        status: ThreadStatus::Open,
        intent: ThreadIntent::Comment,
        severity: None,
        origin: None,
        source_id: None,
        labels: None,
        proposal: None,
        agent_state: None,
        created_at: 0,
        updated_at: 0,
        extra: Default::default(),
    };
    location_label(&thread)
}

// ---- The gutter column: markers and **+** ----

/// The review column of an editor's gutter (`co-review:` hook in `editor_gutter`): a marker per thread (click to
/// fold or unfold it, hover for its first message) and a **+** on the hovered line; press **+** to comment on the
/// line, drag over lines to ask about them.
pub fn gutter_column(
    window_tab: Rc<WindowTabData>,
    e_data: RwSignal<EditorData>,
    icon_padding: f32,
    gutter_width: Memo<f64>,
) -> impl View {
    let (editor, doc_signal, config) =
        e_data.with_untracked(|e| (e.editor.clone(), e.doc_signal(), e.common.config));
    let scroll_delta = editor.scroll_delta;
    let viewport = editor.viewport;
    let screen_lines = editor.screen_lines;
    let hovered = create_rw_signal(None::<usize>);
    let dragging = create_rw_signal(None::<(usize, usize)>);
    let threads = create_memo(move |_| {
        let doc = doc_signal.get();
        let path = doc.content.with(|content| content.path().cloned());
        path.and_then(|path| doc_threads(&path))
            .map(|threads| (threads.items, threads.summaries))
    });
    let items = create_memo(move |_| {
        threads
            .get()
            .map(|(items, _)| items.get())
            .unwrap_or_default()
    });
    let folded = folded_signal(&window_tab);

    // The buffer line at a y of the gutter (first visual line of a line only).
    let line_at = move |y: f64| -> Option<usize> {
        let line_height = config.get_untracked().editor.line_height() as f64;
        let y = y + viewport.get_untracked().y0;
        screen_lines.with_untracked(|screen_lines| {
            screen_lines.iter_line_info().find_map(|info| {
                (info.vline_y <= y && y < info.vline_y + line_height && info.vline_info.rvline.line_index == 0)
                    .then_some(info.vline_info.rvline.line)
            })
        })
    };
    let line_y = move |line: usize| -> Option<f64> {
        let y = screen_lines.with(|screen_lines| screen_lines.info_for_line(line).map(|info| info.y))?;
        Some(y - viewport.get().y0)
    };

    let markers = dyn_stack(
        move || items.get(),
        |item| item.clone(),
        move |item| {
            let key = item.key.clone();
            let summary = move || {
                threads
                    .get_untracked()
                    .and_then(|(_, summaries)| summaries.with(|summaries| summaries.get(&key).cloned()))
                    .unwrap_or_default()
            };
            let color = match item.kind {
                MarkerKind::Question => LapceColor::EDITOR_LINK,
                MarkerKind::Proposed => LapceColor::LAPCE_WARN,
                MarkerKind::Resolved | MarkerKind::Draft => LapceColor::EDITOR_DIM,
                MarkerKind::Comment => LapceColor::LAPCE_ICON_ACTIVE,
            };
            let line = item.line;
            tooltip(
                container(svg(move || config.get().ui_svg(LapceIcons::REVIEW)).style(move |s| {
                    let config = config.get();
                    let size = config.ui.icon_size() as f32;
                    s.size(size, size).color(config.color(color))
                })),
                move || {
                    let summary = summary.clone();
                    label(summary).style(move |s| {
                        let config = config.get();
                        s.max_width(400.0)
                            .padding_horiz(8.0)
                            .padding_vert(4.0)
                            .border(1.0)
                            .border_radius(6.0)
                            .border_color(config.color(LapceColor::LAPCE_BORDER))
                            .background(config.color(LapceColor::TOOLTIP_BACKGROUND))
                            .color(config.color(LapceColor::TOOLTIP_FOREGROUND))
                    })
                },
            )
            .style(move |s| {
                let config = config.get();
                let top = line_y(line);
                s.absolute()
                    .width(config.ui.icon_size() as f32 + icon_padding * 2.0)
                    .height(config.editor.line_height() as f32)
                    .justify_center()
                    .items_center()
                    .margin_top(top.unwrap_or(-1000.0) as f32)
                    .apply_if(top.is_none(), |s| s.hide())
            })
        },
    )
    .style(|s| s.absolute().size_full());

    let plus = label(|| "+").style(move |s| {
        let config = config.get();
        let line = hovered.get();
        let on_marker = line.is_some_and(|line| items.with(|items| items.iter().any(|item| item.line == line)));
        let top = line.and_then(line_y);
        s.absolute()
            .width(config.ui.icon_size() as f32 + icon_padding * 2.0)
            .height(config.editor.line_height() as f32)
            .justify_center()
            .items_center()
            .font_bold()
            .color(config.color(LapceColor::EDITOR_LINK))
            .margin_top(top.unwrap_or(-1000.0) as f32)
            .apply_if(top.is_none() || on_marker || dragging.get().is_some(), |s| s.hide())
            .selectable(false)
    });
    let selection = empty().style(move |s| {
        let config = config.get();
        let Some((start, end)) = dragging.get() else {
            return s.hide();
        };
        let (first, last) = (start.min(end), start.max(end));
        let (Some(top), Some(bottom)) = (line_y(first), line_y(last)) else {
            return s.hide();
        };
        let line_height = config.editor.line_height() as f64;
        s.absolute()
            .margin_left(icon_padding + 2.0)
            .width(4.0)
            .margin_top(top as f32)
            .height((bottom - top + line_height) as f32)
            .border_radius(2.0)
            .background(config.color(LapceColor::EDITOR_LINK))
    });

    let press_tab = window_tab.clone();
    container((markers, selection, plus))
        .on_event_cont(EventListener::PointerMove, move |event| {
            if let Event::PointerMove(pointer) = event {
                let line = line_at(pointer.pos.y);
                if hovered.get_untracked() != line {
                    hovered.set(line);
                }
                if let (Some((start, _)), Some(line)) = (dragging.get_untracked(), line) {
                    dragging.set(Some((start, line)));
                }
            }
        })
        .on_event_cont(EventListener::PointerLeave, move |_| hovered.set(None))
        // On top of the line numbers, so it scrolls the editor as the gutter under it does.
        .on_event_stop(EventListener::PointerWheel, move |event| {
            if let Event::PointerWheel(pointer) = event {
                scroll_delta.set(pointer.delta);
            }
        })
        .on_event_stop(EventListener::PointerDown, move |event| {
            let Event::PointerDown(pointer) = event else {
                return;
            };
            let Some(line) = line_at(pointer.pos.y) else {
                return;
            };
            // A marker's line: fold or unfold its threads.
            let on_line: Vec<Item> = items.with_untracked(|items| {
                items.iter().filter(|item| item.line == line).cloned().collect()
            });
            if !on_line.is_empty() {
                let fold = on_line.iter().any(|item| !item.folded);
                folded.update(|folded| {
                    for item in &on_line {
                        if let ItemKey::Thread { thread_id, .. } = &item.key {
                            folded.insert(thread_id.clone(), fold);
                        }
                    }
                });
                return;
            }
            dragging.set(Some((line, line)));
        })
        .on_event_stop(EventListener::PointerUp, move |_| {
            let Some((start, end)) = dragging.get_untracked() else {
                return;
            };
            dragging.set(None);
            let (first, last) = (start.min(end), start.max(end));
            let intent = if first == last { ThreadIntent::Comment } else { ThreadIntent::Question };
            let doc = doc_signal.get_untracked();
            if let Some(location) = lines_location(&doc, first, last) {
                start_draft(&press_tab, location, intent);
            }
        })
        // From the + column across the line numbers, so clicking a line number starts a comment on that line
        // (dragging over several, a question about them), leaving the code-action light bulb at the right edge.
        .style(move |s| {
            let config = config.get();
            let width = config.ui.icon_size() as f32 + icon_padding * 2.0;
            let left = width - 8.0;
            s.absolute()
                .width((gutter_width.get() as f32 - left - width).max(width))
                .height_full()
                .margin_left(left)
                .cursor(CursorStyle::Pointer)
        })
        .debug_name("Review Gutter")
}

// ---- Starting threads ----

/// Opens a draft at `location` and focuses it.
pub fn start_draft(window_tab: &WindowTabData, location: CodeLocation, intent: ThreadIntent) {
    drafts::add(window_tab, location, intent, |id| {
        FOCUS.with(|focus| *focus.borrow_mut() = Some(id));
    });
}

/// The file URI threads use for a path: resolved through symbolic links.
fn file_uri(path: &Path) -> String {
    review_store::path_to_file_uri(&review_store::real_path(path).to_string_lossy())
}

fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

/// Lines `first`..=`last` of the document as a location: a line or a range of whole lines.
pub fn lines_location(doc: &Doc, first: usize, last: usize) -> Option<CodeLocation> {
    let path = doc_path(doc)?;
    let (text, last_len) = doc.buffer.with_untracked(|buffer| {
        let last_line = buffer.line_content(last);
        let last_line = last_line.trim_end_matches(['\n', '\r']).to_string();
        let start = buffer.offset_of_line(first);
        let end = buffer.offset_of_line(last) + last_line.len();
        (buffer.slice_to_cow(start..end).to_string(), utf16_len(&last_line))
    });
    Some(CodeLocation {
        kind: if first == last { LocationKind::Line } else { LocationKind::Range },
        uri: Some(file_uri(&path)),
        range: Some(Range {
            start: Position { line: first as u32, character: 0 },
            end: Position { line: last as u32, character: last_len },
        }),
        symbol: None,
        anchor: Some(CodeAnchor { text, before: None, after: None, syntax: None }),
        doc_anchor: None,
        patch_anchor: None,
        tracked: None,
        extra: Default::default(),
    })
}

/// **Comment** (`Cmd+Alt+M`) and **Ask Agent** (`Cmd+Alt+A`): a draft on the active editor's selection — on the
/// symbol when the selection is one identifier, else on the selected lines (the cursor's line without a selection).
pub fn comment_on_selection(window_tab: &WindowTabData, intent: ThreadIntent) {
    let Some(editor) = window_tab.main_split.active_editor.get_untracked() else {
        return;
    };
    let doc = editor.doc();
    let Some(path) = doc_path(&doc) else {
        return;
    };
    let (start, end) = editor.cursor().with_untracked(|cursor| selected_range(&cursor.mode));
    let location = doc.buffer.with_untracked(|buffer| {
        let end = end.min(buffer.len());
        let (first, first_col) = buffer.offset_to_line_col(start);
        let (last, last_col) = buffer.offset_to_line_col(end);
        if start == end {
            return None;
        }
        let selected = buffer.slice_to_cow(start..end).to_string();
        let line_text = |line: usize| buffer.line_content(line).to_string();
        let column = |line: usize, col: usize| {
            let text = line_text(line);
            utf16_len(text.get(..col).unwrap_or(&text))
        };
        let symbol = is_identifier(&selected).then(|| {
            let language = path
                .extension()
                .map(|extension| extension.to_string_lossy().into_owned())
                .unwrap_or_default();
            ReviewCore::get()
                .and_then(|core| core.services.syntax.enclosing_symbol(&language, &buffer.to_string(), first as u32))
                .filter(|enclosing| enclosing.rsplit('.').next() == Some(selected.as_str()))
                .unwrap_or_else(|| selected.clone())
        });
        Some(CodeLocation {
            kind: if symbol.is_some() {
                LocationKind::Symbol
            } else if first == last {
                LocationKind::Line
            } else {
                LocationKind::Range
            },
            uri: Some(file_uri(&path)),
            range: Some(Range {
                start: Position { line: first as u32, character: column(first, first_col) },
                end: Position { line: last as u32, character: column(last, last_col) },
            }),
            symbol,
            anchor: Some(CodeAnchor { text: selected, before: None, after: None, syntax: None }),
            doc_anchor: None,
            patch_anchor: None,
            tracked: None,
            extra: Default::default(),
        })
    });
    let location = match location {
        Some(location) => Some(location),
        None => {
            let line = doc.buffer.with_untracked(|buffer| buffer.line_of_offset(start));
            lines_location(&doc, line, line)
        }
    };
    if let Some(location) = location {
        start_draft(window_tab, location, intent);
    }
}

/// The selected offsets (`start..end`) of the first selection; empty without a selection.
pub fn selected_range(mode: &CursorMode) -> (usize, usize) {
    match mode {
        CursorMode::Normal(offset) => (*offset, *offset),
        CursorMode::Visual { start, end, .. } => ((*start).min(*end), (*start).max(*end) + 1),
        CursorMode::Insert(selection) => selection
            .first()
            .map_or((0, 0), |region| (region.min(), region.max())),
    }
}

/// Whether the selected text is one identifier: a comment on it is a comment on a symbol.
pub fn is_identifier(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        && !text.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// **Add Review Comment on Symbol**: a draft on the innermost symbol around the cursor, as the language server lists
/// the file's symbols (its whole range; anchored by its declaration line).
pub fn comment_on_symbol(window_tab: &WindowTabData) {
    let Some(editor) = window_tab.main_split.active_editor.get_untracked() else {
        return;
    };
    let doc = editor.doc();
    let Some(path) = doc_path(&doc) else {
        return;
    };
    let offset = editor.cursor().with_untracked(|cursor| cursor.offset());
    let line = doc.buffer.with_untracked(|buffer| buffer.line_of_offset(offset)) as u32;
    let tab = super::commands::ReviewTab::of(window_tab);
    let window_tab = Rc::new(window_tab.clone());
    let uri = file_uri(&path);
    let proxy = window_tab.common.proxy.clone();
    let send = floem::ext_event::create_ext_action(window_tab.scope, move |result| {
        let symbols = match result {
            Ok(lapce_rpc::proxy::ProxyResponse::GetDocumentSymbols { resp }) => resp,
            _ => lsp_types::DocumentSymbolResponse::Nested(Vec::new()),
        };
        let Some(symbol) = enclosing_symbol(&symbols, line) else {
            tab.info("No symbol found at the cursor. Language support may still be starting (or the language has no plugin).");
            return;
        };
        let declaration = symbol.selection_range.start.line as usize;
        let anchor = doc.buffer.with_untracked(|buffer| {
            (declaration <= buffer.last_line())
                .then(|| buffer.line_content(declaration).trim_end_matches(['\n', '\r']).to_string())
        });
        let location = CodeLocation {
            kind: LocationKind::Symbol,
            uri: Some(uri),
            range: Some(Range {
                start: Position { line: symbol.range.start.line, character: symbol.range.start.character },
                end: Position { line: symbol.range.end.line, character: symbol.range.end.character },
            }),
            symbol: Some(symbol.path),
            anchor: anchor.map(|text| CodeAnchor { text, before: None, after: None, syntax: None }),
            doc_anchor: None,
            patch_anchor: None,
            tracked: None,
            extra: Default::default(),
        };
        start_draft(&window_tab, location, ThreadIntent::Comment);
    });
    proxy.get_document_symbols(path, send);
}

/// A symbol around a line, with its dotted path (`OrderService.create`).
#[derive(Debug, PartialEq)]
pub struct EnclosingSymbol {
    pub path: String,
    pub range: lsp_types::Range,
    pub selection_range: lsp_types::Range,
}

/// The innermost symbol whose range contains `line` (0-based). Locals, constants and literals are not part of a
/// symbol path and are never the symbol (as in the TypeScript `findEnclosing`).
pub fn enclosing_symbol(
    symbols: &lsp_types::DocumentSymbolResponse,
    line: u32,
) -> Option<EnclosingSymbol> {
    use lsp_types::{DocumentSymbolResponse, SymbolKind};
    let ignored = |kind: SymbolKind| {
        [
            SymbolKind::VARIABLE,
            SymbolKind::CONSTANT,
            SymbolKind::STRING,
            SymbolKind::NUMBER,
            SymbolKind::BOOLEAN,
            SymbolKind::ARRAY,
            SymbolKind::OBJECT,
            SymbolKind::KEY,
            SymbolKind::NULL,
            SymbolKind::TYPE_PARAMETER,
        ]
        .contains(&kind)
    };
    let contains = |range: &lsp_types::Range| range.start.line <= line && line <= range.end.line;
    fn nested(
        symbols: &[lsp_types::DocumentSymbol],
        parents: &[String],
        contains: &dyn Fn(&lsp_types::Range) -> bool,
        ignored: &dyn Fn(SymbolKind) -> bool,
    ) -> Option<EnclosingSymbol> {
        for symbol in symbols.iter().filter(|symbol| contains(&symbol.range)) {
            let skip = ignored(symbol.kind);
            let mut path = parents.to_vec();
            if !skip {
                path.push(symbol.name.clone());
            }
            let children = symbol.children.as_deref().unwrap_or_default();
            if let Some(child) = nested(children, &path, contains, ignored) {
                return Some(child);
            }
            if !skip {
                return Some(EnclosingSymbol {
                    path: path.join("."),
                    range: symbol.range,
                    selection_range: symbol.selection_range,
                });
            }
        }
        None
    }
    match symbols {
        DocumentSymbolResponse::Nested(symbols) => nested(symbols, &[], &contains, &ignored),
        // A flat list: the smallest symbol around the line, under its container.
        DocumentSymbolResponse::Flat(symbols) => symbols
            .iter()
            .filter(|symbol| !ignored(symbol.kind) && contains(&symbol.location.range))
            .min_by_key(|symbol| {
                let range = symbol.location.range;
                (range.end.line.saturating_sub(range.start.line), u32::MAX - range.start.line)
            })
            .map(|symbol| EnclosingSymbol {
                path: match &symbol.container_name {
                    Some(container) if !container.is_empty() => format!("{container}.{}", symbol.name),
                    _ => symbol.name.clone(),
                },
                range: symbol.location.range,
                selection_range: symbol.location.range,
            }),
    }
}

/// **Comment on File / Folder** from the file explorer: a draft in the Review panel (a file opens in the editor
/// too). The workspace folder itself is a comment on the repository.
pub fn comment_on_path(window_tab: &WindowTabData, path: &Path, intent: ThreadIntent) {
    let is_root = window_tab
        .review
        .root
        .as_deref()
        .is_some_and(|root| review_store::real_path(path) == root);
    let kind = if is_root {
        LocationKind::Repository
    } else if path.is_dir() {
        LocationKind::Directory
    } else {
        LocationKind::File
    };
    let location = CodeLocation {
        kind,
        uri: (!is_root).then(|| file_uri(path)),
        range: None,
        symbol: None,
        anchor: None,
        doc_anchor: None,
        patch_anchor: None,
        tracked: None,
        extra: Default::default(),
    };
    start_draft(window_tab, location, intent);
    window_tab.show_panel(crate::panel::kind::PanelKind::Review);
}

// ---- Moving between threads, folding ----

/// **Next / Previous Comment** (`Cmd+Alt+↓` / `Cmd+Alt+↑`): the next thread of the active review on code after (or
/// before) the cursor, across files in path order; it opens there, unfolded.
pub fn go_to_next_thread(window_tab: &WindowTabData, forward: bool) {
    let Some(review) = window_tab.review.active_review_untracked() else {
        return;
    };
    let mut places: Vec<(PathBuf, usize, String)> = review
        .threads
        .iter()
        .filter(|thread| thread.status != ThreadStatus::Resolved)
        .filter_map(|thread| {
            let path = thread.location.uri.as_deref().and_then(review_store::file_uri_to_path)?;
            let line = live_line(&path, &thread.id)
                .or_else(|| thread.location.range.as_ref().map(|range| range.start.line as usize))?;
            Some((PathBuf::from(path), line, thread.id.clone()))
        })
        .collect();
    if places.is_empty() {
        return;
    }
    places.sort();
    let here = window_tab.main_split.active_editor.get_untracked().and_then(|editor| {
        let doc = editor.doc();
        let path = doc_path(&doc)?;
        let offset = editor.cursor().with_untracked(|cursor| cursor.offset());
        let line = doc.buffer.with_untracked(|buffer| buffer.line_of_offset(offset));
        Some((review_store::real_path(&path), line))
    });
    let key = |place: &(PathBuf, usize, String)| (review_store::real_path(&place.0), place.1);
    let target = match &here {
        Some(here) if forward => places.iter().find(|place| key(place) > *here).or(places.first()),
        Some(here) => places.iter().rev().find(|place| key(place) < *here).or(places.last()),
        None => places.first(),
    };
    let Some((path, line, thread_id)) = target.cloned() else {
        return;
    };
    folded_signal(window_tab).update(|folded| {
        folded.insert(thread_id, false);
    });
    open_location(window_tab, path, Some(line));
}

fn live_line(path: &str, thread_id: &str) -> Option<usize> {
    let threads = DOCS.with(|docs| {
        docs.borrow()
            .iter()
            .find(|(doc, _)| review_store::real_path(doc) == review_store::real_path(Path::new(path)))
            .map(|(_, threads)| threads.clone())
    })?;
    threads.items.with_untracked(|items| {
        items
            .iter()
            .find(|item| matches!(&item.key, ItemKey::Thread { thread_id: id, .. } if id == thread_id))
            .map(|item| item.line)
    })
}

/// **Collapse / Expand All Comments**: every thread of the active review folded to its gutter marker, or open.
pub fn fold_all(window_tab: &WindowTabData, fold: bool) {
    let Some(review) = window_tab.review.active_review_untracked() else {
        return;
    };
    folded_signal(window_tab).update(|folded| {
        for thread in &review.threads {
            folded.insert(thread.id.clone(), fold);
        }
    });
}

/// Unfolds a thread (the panel opening it at its code).
pub fn unfold(window_tab: &WindowTabData, thread_id: &str) {
    folded_signal(window_tab).update(|folded| {
        folded.insert(thread_id.to_string(), false);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(deprecated)]
    fn symbol(
        name: &str,
        kind: lsp_types::SymbolKind,
        lines: (u32, u32),
        children: Vec<lsp_types::DocumentSymbol>,
    ) -> lsp_types::DocumentSymbol {
        let range = lsp_types::Range {
            start: lsp_types::Position { line: lines.0, character: 0 },
            end: lsp_types::Position { line: lines.1, character: 1 },
        };
        lsp_types::DocumentSymbol {
            name: name.to_string(),
            detail: None,
            kind,
            tags: None,
            deprecated: None,
            range,
            selection_range: range,
            children: Some(children),
        }
    }

    #[test]
    fn the_innermost_symbol_is_commented_on() {
        use lsp_types::SymbolKind;
        let symbols = lsp_types::DocumentSymbolResponse::Nested(vec![symbol(
            "OrderService",
            SymbolKind::CLASS,
            (0, 20),
            vec![symbol(
                "create",
                SymbolKind::METHOD,
                (2, 10),
                vec![symbol("total", SymbolKind::VARIABLE, (4, 4), Vec::new())],
            )],
        )]);
        let found = |line| enclosing_symbol(&symbols, line).map(|symbol| symbol.path);
        assert_eq!(found(4).as_deref(), Some("OrderService.create"));
        assert_eq!(found(15).as_deref(), Some("OrderService"));
        assert_eq!(found(30), None);
    }

    fn job(start: Start) -> Job {
        Job {
            key: ItemKey::Draft("d1".to_string()),
            basis: "b".to_string(),
            start,
        }
    }

    #[test]
    fn typing_above_moves_the_thread_down() {
        let before = "fn a() {}\nfn b() {}\n";
        let tracked = review_anchor::range_in(before, "v1", 10, 19, AnchorStatus::Active, None);
        // Several edits since the last placement (a background placement was still running).
        let after = "// new\n// newer\nfn a() {}\nfn b() {}\n";
        let placed = placements(Some(before), after, vec![job(Start::Tracked(Box::new(tracked)))], None);
        let live = &placed[0].1;
        assert_eq!(live.tracked.as_ref().map(|tracked| tracked.status), Some(AnchorStatus::Active));
        assert_eq!(live.line, Some(3));
    }

    #[test]
    fn deleted_code_takes_the_thread_off_the_code() {
        let before = "fn a() {}\nfn b() {}\n";
        let tracked = review_anchor::range_in(before, "v1", 10, 19, AnchorStatus::Active, None);
        let placed = placements(Some(before), "fn a() {}\n", vec![job(Start::Tracked(Box::new(tracked)))], None);
        assert_eq!(placed[0].1.line, None);
    }

    fn draft_entry(body: &str) -> Entry {
        Entry {
            key: ItemKey::Draft("d1".to_string()),
            location: serde_json::from_value(serde_json::json!({ "kind": "line" })).unwrap(),
            folded: false,
            kind: MarkerKind::Draft,
            summary: summary(body),
        }
    }

    /// Typing in a draft changes its summary only: the items (and so the editor's layout and the draft's place)
    /// stay as they are.
    #[test]
    fn typing_in_a_draft_changes_no_item() {
        let live = HashMap::from([(
            ItemKey::Draft("d1".to_string()),
            Live { basis: "b".to_string(), tracked: None, line: Some(4) },
        )]);
        let (before, before_summaries) = placed_items(&[draft_entry("Th")], &live);
        let (after, after_summaries) = placed_items(&[draft_entry("This needs a test")], &live);
        assert_eq!(before, after);
        assert_eq!(after[0].line, 4);
        assert_ne!(before_summaries, after_summaries);
    }

    fn rows(layout_lines: usize, on_screen: &[(usize, f64)], line_ends_screen: bool) -> ItemRows<'_> {
        ItemRows {
            rows_total: 5,
            rows_before: 0,
            layout_lines,
            on_screen,
            line_ends_screen,
            line_height: 20.0,
        }
    }

    #[test]
    fn a_thread_sits_on_its_rows_and_keeps_its_place_while_the_editor_lays_out_again() {
        // The line's text (index 0) and its five rows (1..=5), from y 100.
        let screen: Vec<(usize, f64)> = (0..6).map(|index| (index, 100.0 + index as f64 * 20.0)).collect();
        assert_eq!(item_top(rows(6, &screen, false)), Placed::At(120.0));
        // Scrolled so that only the last two rows show: the view starts above the screen.
        assert_eq!(item_top(rows(6, &screen[4..], false)), Placed::At(120.0));
        // The layout doesn't have the rows yet, or the screen lines are from before them: not hidden.
        assert_eq!(item_top(rows(1, &screen, false)), Placed::Unsettled);
        assert_eq!(item_top(rows(6, &screen[..1], false)), Placed::Unsettled);
        // Off screen: the line isn't there, or its rows are below the screen's end.
        assert_eq!(item_top(rows(6, &[], false)), Placed::OffScreen);
        assert_eq!(item_top(rows(6, &screen[..1], true)), Placed::OffScreen);
    }

    #[test]
    fn a_first_placement_uses_the_stored_range() {
        let location: CodeLocation = serde_json::from_value(serde_json::json!({
            "kind": "line",
            "uri": "file:///w/a.rs",
            "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 9 } },
        }))
        .unwrap();
        let placed = placements(None, "fn a() {}\nfn b() {}\n", vec![job(Start::Location(Box::new(location)))], None);
        assert_eq!(placed[0].1.line, Some(1));
        assert_eq!(placed[0].1.tracked.as_ref().map(|tracked| tracked.status), Some(AnchorStatus::Active));
    }
}
