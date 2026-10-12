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
//! `ambiguous`: out of scope, never shown on code).

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
        RwSignal, Scope, SignalGet, SignalUpdate, SignalWith, create_memo,
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
use review_anchor::{AnchorStatus, Edit, TrackedRange};
use review_model::{
    CodeAnchor, CodeLocation, LocationKind, Position, Range, Review, ReviewThread,
    ThreadIntent, ThreadStatus,
};

use super::{
    ReviewCore, drafts,
    thread::{
        ThreadViewOptions, location_label, open_location, reply_box, text_button,
        thread_view,
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
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub key: ItemKey,
    /// The 0-based line it shows under (the last line of its range).
    pub line: usize,
    /// Only the gutter marker shows.
    pub folded: bool,
    pub kind: MarkerKind,
    /// The first message (or the draft's text), for the gutter marker's hover.
    pub summary: String,
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

/// The tracked range of a thread or draft in the editor's current text, and the stored target it came from.
struct Live {
    basis: String,
    tracked: Option<TrackedRange>,
    /// The line to use without a tracked range.
    fallback_line: Option<usize>,
}

/// The threads and drafts of one open file (shared by every editor on it).
pub struct DocThreads {
    uri: String,
    entries: RefCell<Vec<Entry>>,
    live: RefCell<HashMap<ItemKey, Live>>,
    pub items: RwSignal<Vec<Item>>,
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
                    entries: RefCell::default(),
                    live: RefCell::default(),
                    items: cx.create_rw_signal(Vec::new()),
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

/// The document changed (`co-review:` hook in `Doc::apply_deltas`): threads follow the edit before it is saved.
/// `before` is the text before the edit.
pub fn doc_edited(doc: &Doc, before: &Rope) {
    let Some(path) = doc_path(doc) else {
        return;
    };
    let Some(threads) = doc_threads(&path) else {
        return;
    };
    if threads.live.borrow().is_empty() {
        return;
    }
    let before = before.to_string();
    let after = doc.buffer.with_untracked(|buffer| buffer.to_string());
    let edit = single_edit(&before, &after);
    for live in threads.live.borrow_mut().values_mut() {
        if let Some(tracked) = &live.tracked {
            live.tracked = Some(review_anchor::track(
                tracked,
                Some(&before),
                &after,
                "live",
                Some(std::slice::from_ref(&edit)),
            ));
        }
    }
    threads.place(doc, &after);
}

/// The one replacement that turns `before` into `after` (common start and end kept), in UTF-16 units.
fn single_edit(before: &str, after: &str) -> Edit {
    let (a, b): (Vec<u16>, Vec<u16>) = (before.encode_utf16().collect(), after.encode_utf16().collect());
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let max_suffix = a.len().min(b.len()) - prefix;
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    Edit {
        from: prefix,
        to: a.len() - suffix,
        insert: String::from_utf16_lossy(&b[prefix..b.len() - suffix]),
    }
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
    fn refresh(&self, doc: &Doc, review: Option<&Review>, drafts: &[super::workspace::ReviewDraft], folded: &HashMap<String, bool>) {
        if !doc.loaded.get_untracked() {
            // Placed once the text is there; nothing is tracked against an empty buffer.
            self.live.borrow_mut().clear();
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
        let text = doc.buffer.with_untracked(|buffer| buffer.to_string());
        {
            let snapshots = review.and_then(|review| {
                ReviewCore::get().map(|core| {
                    review_store::snapshots::snapshots_dir(
                        &core.services.store.workspace_dir(&review.workspace_root),
                    )
                })
            });
            let mut live = self.live.borrow_mut();
            live.retain(|key, _| entries.iter().any(|entry| &entry.key == key));
            for entry in &entries {
                let basis = basis(&entry.location);
                if live.get(&entry.key).is_some_and(|live| live.basis == basis) {
                    continue;
                }
                live.insert(
                    entry.key.clone(),
                    Live {
                        tracked: live_range(&entry.location, &text, snapshots.as_deref()),
                        fallback_line: entry.location.range.as_ref().map(end_line),
                        basis,
                    },
                );
            }
        }
        tracing::debug!(
            "{} review threads and drafts for {} (review {:?})",
            entries.len(),
            self.uri,
            review.map(|review| review.id.as_str())
        );
        *self.entries.borrow_mut() = entries;
        self.place(doc, &text);
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

    /// Places the entries by their live ranges in `text` (the editor's text); out-of-scope ones are left out.
    fn place(&self, doc: &Doc, text: &str) {
        let live = self.live.borrow();
        let mut items: Vec<Item> = self
            .entries
            .borrow()
            .iter()
            .filter_map(|entry| {
                let live = live.get(&entry.key)?;
                let line = match &live.tracked {
                    Some(tracked) => match tracked.status {
                        AnchorStatus::Removed | AnchorStatus::Ambiguous => return None,
                        AnchorStatus::Active | AnchorStatus::Modified => {
                            let end = if tracked.to > tracked.from { tracked.to - 1 } else { tracked.from };
                            review_anchor::position_at(text, end as usize).0
                        }
                    },
                    None => live.fallback_line?,
                };
                Some(Item {
                    key: entry.key.clone(),
                    line,
                    folded: entry.folded,
                    kind: entry.kind,
                    summary: entry.summary.clone(),
                })
            })
            .collect();
        drop(live);
        items.sort_by_key(|item| item.line);
        let changed = self.items.with_untracked(|current| *current != items);
        if changed {
            tracing::debug!(
                "review threads in {}: {:?}",
                self.uri,
                items.iter().map(|item| (item.line, item.folded)).collect::<Vec<_>>()
            );
            self.items.set(items);
            doc.clear_text_cache();
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
            self.rows.update(|current| {
                current.insert(key.clone(), rows);
            });
            doc.clear_text_cache();
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

/// The threads and drafts of the editor's file, on the rows reserved for them (`co-review:` hook in
/// `editor_container_view`). Absolutely positioned over the editor; only the threads take pointer events.
pub fn editor_threads(window_tab: Rc<WindowTabData>, e_data: RwSignal<EditorData>) -> impl View {
    let doc_signal = e_data.with_untracked(|editor| editor.doc_signal());
    let origin = create_rw_signal(Point::ZERO);
    let size = create_rw_signal(Rect::ZERO);
    dyn_container(
        move || doc_signal.with(|doc| doc.content.with(|content| content.path().cloned())),
        move |path| match path {
            Some(path) => doc_overlay(window_tab.clone(), e_data, path, origin, size).into_any(),
            None => empty().into_any(),
        },
    )
    .on_move(move |point| origin.set(point))
    .on_resize(move |rect| size.set(rect))
    .style(|s| s.absolute().size_full())
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
            container(content)
                .on_resize(move |rect| {
                    let line_height = config.get_untracked().editor.line_height() as f64;
                    let doc = doc_signal.get_untracked();
                    measure_threads.measured(&doc, &measured_key, rect.height(), line_height);
                })
                .on_event_stop(EventListener::PointerDown, |_| {})
                .style(move |s| {
                    let config = config.get();
                    let line_height = config.editor.line_height() as f64;
                    let viewport = viewport.get();
                    let line = items.with(|items| {
                        items.iter().find(|item| item.key == key).map(|item| item.line)
                    });
                    let top = line.and_then(|line| {
                        let rows_total = threads.rows_at(line);
                        let layout_lines = editor.text_layout(line).line_count();
                        let first_row = layout_lines.checked_sub(rows_total)?;
                        let y = screen_lines.with(|screen_lines| {
                            screen_lines.iter_line_info().find_map(|info| {
                                let rvline = info.vline_info.rvline;
                                (rvline.line == line && rvline.line_index >= first_row).then(|| {
                                    info.vline_y
                                        - (rvline.line_index - first_row) as f64 * line_height
                                })
                            })
                        })?;
                        let before = threads.rows_before(&key, line) as f64 * line_height;
                        Some(y + before - viewport.y0 + GAP / 2.0)
                    });
                    let left = (content_origin.get().x - origin.get().x).max(0.0) + 8.0;
                    let width = (size.get().width() - left - 24.0).clamp(200.0, 960.0);
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
    let has_agent = {
        let review = window_tab.review.clone();
        move || {
            draft.with(|d| {
                d.as_ref().is_some_and(|d| {
                    review.reviews.with(|reviews| {
                        reviews
                            .iter()
                            .any(|review| review.id == d.review_id && review.agent.is_some())
                    })
                })
            })
        }
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
    let ask = send(ThreadIntent::Question, "Ask Agent", false);
    let has_agent_for_style = has_agent;
    floem::views::v_stack((
        h_stack((title, empty().style(|s| s.flex_grow(1.0_f32)), discard)).style(|s| s.width_full().items_center()),
        input.style(|s| s.margin_top(6.0)),
        h_stack((
            empty().style(|s| s.flex_grow(1.0_f32)),
            ask.style(move |s| s.apply_if(!has_agent_for_style(), |s| s.hide())),
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
) -> impl View {
    let (editor, doc_signal, config) =
        e_data.with_untracked(|e| (e.editor.clone(), e.doc_signal(), e.common.config));
    let viewport = editor.viewport;
    let screen_lines = editor.screen_lines;
    let hovered = create_rw_signal(None::<usize>);
    let dragging = create_rw_signal(None::<(usize, usize)>);
    let items = create_memo(move |_| {
        let doc = doc_signal.get();
        let path = doc.content.with(|content| content.path().cloned());
        path.and_then(|path| doc_threads(&path))
            .map(|threads| threads.items.get())
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
        |item| (item.key.clone(), item.line, item.folded, item.kind, item.summary.clone()),
        move |item| {
            let summary = item.summary.clone();
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
                    label(move || summary.clone()).style(move |s| {
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
        .style(move |s| {
            let config = config.get();
            let width = config.ui.icon_size() as f32 + icon_padding * 2.0;
            s.absolute()
                .width(width)
                .height_full()
                .margin_left(width - 8.0)
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
    let (start, end) = editor.cursor().with_untracked(|cursor| match &cursor.mode {
        CursorMode::Normal(offset) => (*offset, *offset),
        CursorMode::Visual { start, end, .. } => ((*start).min(*end), (*start).max(*end) + 1),
        CursorMode::Insert(selection) => selection
            .first()
            .map_or((0, 0), |region| (region.min(), region.max())),
    });
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
        let identifier = !selected.is_empty()
            && selected.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$')
            && !selected.chars().next().is_some_and(|c| c.is_ascii_digit());
        let symbol = identifier.then(|| {
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

    #[test]
    fn single_edit_is_the_changed_middle() {
        let edit = single_edit("let a = 1;\nlet b = 2;\n", "let a = 1;\nlet bb = 2;\n");
        assert_eq!((edit.from, edit.to, edit.insert.as_str()), (16, 16, "b"));
        let edit = single_edit("abc", "abc");
        assert_eq!((edit.from, edit.to, edit.insert.as_str()), (3, 3, ""));
        let edit = single_edit("aaa", "a");
        assert_eq!((edit.from, edit.to, edit.insert.as_str()), (1, 3, ""));
    }

    #[test]
    fn typing_above_moves_the_thread_down() {
        let before = "fn a() {}\nfn b() {}\n";
        let tracked = review_anchor::range_in(before, "v1", 10, 19, AnchorStatus::Active, None);
        let after = "// new\nfn a() {}\nfn b() {}\n";
        let edit = single_edit(before, after);
        let moved = review_anchor::track(&tracked, Some(before), after, "live", Some(&[edit]));
        assert_eq!(moved.status, AnchorStatus::Active);
        assert_eq!(review_anchor::position_at(after, moved.from as usize).0, 2);
    }
}
