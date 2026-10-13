//! The pull-request page: a change review's patches as a pull request. The `PR.md` header (title, number, branch,
//! commits, description), the files changed with their lines added and removed, and each file's hunks, with threads
//! between the lines they end on. Comments go on the pull request, a file, a line or a range (shift-click a second
//! line), on the old side (removed lines) or the new; on lines they can carry a suggested edit.
//!
//! One virtual list of rows (file headers, diff lines, threads, drafts) renders only what is on screen, so a large
//! diff opens as fast as a small one. Diff lines are read-only rows, not editors: Lapce's diff editor can't be
//! embedded once per file at this scale. Each line steps into the code (`review::code`): a new-side line number opens
//! the head file there, an old-side one the base version; Cmd/Ctrl-click a name goes to its definition and resting on
//! it shows its hover, both from the language server at the head position; **Compare** opens base ↔ head.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    hash::Hash,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use floem::{
    IntoView, View,
    action::{TimerToken, exec_after},
    event::{Event, EventListener},
    kurbo::Point,
    reactive::{
        Memo, ReadSignal, RwSignal, Scope, SignalGet, SignalUpdate, SignalWith,
        create_memo, create_rw_signal,
    },
    style::CursorStyle,
    text::{Attrs, AttrsList, FamilyOwned, TextLayout},
    views::{
        Decorators, VirtualVector, container, dyn_container, empty, label,
        rich_text, scroll, stack, v_stack_from_iter, virtual_stack,
    },
};
use review_bundle::patch::{PatchFile, PatchMeta, PatchRow, parse_patch, parse_patch_meta};
use review_model::{
    CodeAnchor, CodeLocation, LocationKind, PatchAnchor, PatchAnchorType, PatchSide,
    Proposal, ProposalStatus, Review, ReviewThread, ThreadIntent, ThreadOptions,
};

use super::{
    ReviewPage, code, drafts,
    thread::{self, ThreadViewOptions},
    workspace::ReviewDraft,
};
use crate::{
    config::{LapceConfig, color::LapceColor},
    markdown::MarkdownContent,
    window_tab::WindowTabData,
};

/// Files with more diff lines than this start folded, unless they have threads.
const LARGE_FILE_ROWS: usize = 300;

/// One patch of the review directory.
struct LoadedPatch {
    /// The patch file, and its URI as patch threads record it.
    path: PathBuf,
    uri: String,
    name: String,
    files: Vec<Rc<PatchFile>>,
}

/// The review directory's patches and the pull request's metadata (`<patch>.md` or `PR.md`).
struct Loaded {
    meta: Option<PatchMeta>,
    patches: Vec<LoadedPatch>,
    lines: usize,
}

/// What the loader thread sends back (`Rc` isn't `Send`).
struct LoadedParts {
    meta: Option<PatchMeta>,
    patches: Vec<(PathBuf, String, String, Vec<PatchFile>)>,
}

impl From<LoadedParts> for Loaded {
    fn from(parts: LoadedParts) -> Self {
        let patches: Vec<LoadedPatch> = parts
            .patches
            .into_iter()
            .map(|(path, uri, name, files)| LoadedPatch {
                path,
                uri,
                name,
                files: files.into_iter().map(Rc::new).collect(),
            })
            .collect();
        let lines = patches
            .iter()
            .flat_map(|patch| &patch.files)
            .map(|file| file.rows.len())
            .sum();
        Self {
            meta: parts.meta,
            patches,
            lines,
        }
    }
}

fn load(dir: &Path) -> LoadedParts {
    let mut meta = None;
    let mut patches = Vec::new();
    for patch in review_bundle::service::patches_of(dir) {
        let text = match std::fs::read_to_string(&patch) {
            Ok(text) => text,
            Err(error) => {
                tracing::error!("could not read {}: {error}", patch.display());
                continue;
            }
        };
        let name = patch
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if meta.is_none() {
            let slug = review_bundle::payloads::patch_slug(&name).to_string();
            meta = [format!("{slug}.md"), "PR.md".to_string()]
                .iter()
                .find_map(|file| std::fs::read_to_string(dir.join(file)).ok())
                .map(|text| parse_patch_meta(&text));
        }
        let uri = review_store::path_to_file_uri(&patch.to_string_lossy());
        patches.push((patch, uri, name, parse_patch(&text)));
    }
    LoadedParts { meta, patches }
}

/// A comment being written on the page: one of the window's drafts (`drafts.rs`, kept per review and listed in the
/// Review panel) on one of the page's patches. Its text lives in the shared draft.
struct Draft {
    /// The shared draft's id.
    id: String,
    patch: usize,
    anchor: PatchAnchor,
    /// The suggested replacement, one entry per line of the commented source, while suggesting (not kept: drafts
    /// carry no suggestion).
    suggestion: RwSignal<Option<Vec<String>>>,
}

/// A draft's suggested replacement lines, while suggesting.
type Suggestion = RwSignal<Option<Vec<String>>>;

/// Where a shared draft is on the page.
#[derive(Clone, Debug, PartialEq)]
struct PlacedDraft {
    id: String,
    uri: String,
    anchor: PatchAnchor,
}

/// The review's drafts that are on a patch, oldest first.
fn placed_drafts(drafts: &im::Vector<ReviewDraft>, review_id: &str) -> Vec<PlacedDraft> {
    drafts
        .iter()
        .filter(|draft| draft.review_id == review_id && draft.location.kind == LocationKind::Patch)
        .filter_map(|draft| {
            Some(PlacedDraft {
                id: draft.id.clone(),
                uri: draft.location.uri.clone()?,
                anchor: draft.location.patch_anchor.clone()?,
            })
        })
        .collect()
}

/// A draft's location: on the patch at `uri`, at `anchor`.
fn draft_location(uri: String, anchor: PatchAnchor) -> CodeLocation {
    CodeLocation {
        kind: LocationKind::Patch,
        uri: Some(uri),
        range: None,
        symbol: None,
        anchor: Some(CodeAnchor {
            text: anchor.source.clone().unwrap_or_default(),
            before: None,
            after: None,
            syntax: None,
        }),
        doc_anchor: None,
        patch_anchor: Some(anchor),
        tracked: None,
        extra: serde_json::Map::new(),
    }
}

impl Draft {
    fn source_lines(&self) -> Vec<String> {
        self.anchor
            .source
            .as_deref()
            .map(|source| source.split('\n').map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// Suggestions are possible where the original lines are known: lines and ranges.
    fn can_suggest(&self) -> bool {
        self.anchor.source.is_some()
            && matches!(
                self.anchor.anchor_type,
                PatchAnchorType::CodeLine | PatchAnchorType::CodeRange
            )
    }
}

/// A file of a patch, as its rows refer to it.
type FileKey = (usize, String);

#[derive(Clone)]
enum Row {
    /// A patch's name, when the directory has several.
    Patch { patch: usize },
    File {
        patch: usize,
        file: Rc<PatchFile>,
        collapsed: bool,
        threads: usize,
    },
    Code {
        patch: usize,
        file: Rc<PatchFile>,
        index: usize,
        commented: bool,
    },
    Thread { thread_id: String },
    Draft { draft: Rc<Draft> },
    Note { text: String },
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum RowKey {
    Patch(usize),
    File(usize, String, bool, usize),
    Code(usize, String, usize, bool),
    Thread(String),
    Draft(String),
    Note(String),
}

impl Row {
    fn key(&self) -> RowKey {
        match self {
            Row::Patch { patch } => RowKey::Patch(*patch),
            Row::File {
                patch,
                file,
                collapsed,
                threads,
            } => RowKey::File(*patch, file.path.clone(), *collapsed, *threads),
            Row::Code {
                patch,
                file,
                index,
                commented,
            } => RowKey::Code(*patch, file.path.clone(), *index, *commented),
            Row::Thread { thread_id } => RowKey::Thread(thread_id.clone()),
            Row::Draft { draft } => RowKey::Draft(draft.id.clone()),
            Row::Note { text } => RowKey::Note(text.clone()),
        }
    }
}

/// The rows, shared with the virtual list without copying them.
struct Rows(Rc<Vec<Row>>);

impl VirtualVector<Row> for Rows {
    fn total_len(&self) -> usize {
        self.0.len()
    }

    fn slice(&mut self, range: std::ops::Range<usize>) -> impl Iterator<Item = Row> {
        let end = range.end.min(self.0.len());
        let start = range.start.min(end);
        self.0.get(start..end).unwrap_or_default().to_vec().into_iter()
    }
}

fn row_side(row: &PatchRow) -> PatchSide {
    if row.t == "del" {
        PatchSide::Old
    } else {
        PatchSide::New
    }
}

fn row_line(row: &PatchRow) -> Option<u32> {
    if row.t == "del" { row.o } else { row.n }
}

/// The thread's patch anchor when it is on this patch.
fn anchor_on<'a>(thread: &'a ReviewThread, patch: &LoadedPatch) -> Option<&'a PatchAnchor> {
    if thread.location.kind != LocationKind::Patch {
        return None;
    }
    let uri = thread.location.uri.as_deref()?;
    let same = uri == patch.uri
        || review_store::file_uri_to_path(uri).is_some_and(|path| {
            review_store::real_path(Path::new(&path)) == review_store::real_path(&patch.path)
        });
    if !same {
        return None;
    }
    thread.location.patch_anchor.as_ref()
}

/// The diff line a line or range anchor ends on: `(path, side, line)`.
/// A diff line: `(path, old side, line)`.
type LineKey = (String, bool, u32);

fn end_of(anchor: &PatchAnchor) -> Option<LineKey> {
    let line = match anchor.anchor_type {
        PatchAnchorType::CodeLine => anchor.line?,
        PatchAnchorType::CodeRange => anchor.end_line?,
        _ => return None,
    };
    Some((
        anchor.path.clone()?,
        anchor.side == Some(PatchSide::Old),
        line,
    ))
}

/// Every diff line an anchor covers.
fn covered(anchor: &PatchAnchor) -> Vec<LineKey> {
    let Some(path) = anchor.path.clone() else {
        return Vec::new();
    };
    let side = anchor.side == Some(PatchSide::Old);
    let (start, end) = match anchor.anchor_type {
        PatchAnchorType::CodeLine => match anchor.line {
            Some(line) => (line, line),
            None => return Vec::new(),
        },
        PatchAnchorType::CodeRange => match (anchor.start_line, anchor.end_line) {
            (Some(start), Some(end)) => (start.min(end), start.max(end).min(start + 10_000)),
            _ => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    (start..=end).map(|line| (path.clone(), side, line)).collect()
}

/// The rows of the page: per patch (named when there are several) its pull-request threads, then each file's header,
/// file threads and, unless folded, its diff lines with the threads and drafts that end on each.
fn build_rows(
    loaded: &Loaded,
    threads: &[ReviewThread],
    collapsed: &HashSet<FileKey>,
    drafts: &[Rc<Draft>],
) -> Vec<Row> {
    let mut rows = Vec::with_capacity(loaded.lines + loaded.patches.len() * 4);
    for (patch_index, patch) in loaded.patches.iter().enumerate() {
        if loaded.patches.len() > 1 {
            rows.push(Row::Patch { patch: patch_index });
        }
        let on_patch: Vec<(&ReviewThread, &PatchAnchor)> = threads
            .iter()
            .filter_map(|thread| anchor_on(thread, patch).map(|anchor| (thread, anchor)))
            .collect();
        let mut placed: HashSet<&str> = HashSet::new();
        let paths: HashSet<&str> = patch.files.iter().map(|file| file.path.as_str()).collect();
        for (thread, anchor) in &on_patch {
            let elsewhere = anchor.path.as_deref().is_none_or(|path| !paths.contains(path));
            if anchor.anchor_type == PatchAnchorType::Patch || elsewhere {
                rows.push(Row::Thread {
                    thread_id: thread.id.clone(),
                });
                placed.insert(thread.id.as_str());
            }
        }
        for draft in drafts.iter().filter(|draft| {
            draft.patch == patch_index && draft.anchor.anchor_type == PatchAnchorType::Patch
        }) {
            rows.push(Row::Draft {
                draft: Rc::clone(draft),
            });
        }
        let mut ending: HashMap<LineKey, Vec<&str>> = HashMap::new();
        let mut covering: HashSet<LineKey> = HashSet::new();
        for (thread, anchor) in &on_patch {
            if let Some(end) = end_of(anchor) {
                ending.entry(end).or_default().push(thread.id.as_str());
                covering.extend(covered(anchor));
            }
        }
        for draft in drafts.iter().filter(|draft| draft.patch == patch_index) {
            covering.extend(covered(&draft.anchor));
        }
        for file in &patch.files {
            let key = (patch_index, file.path.clone());
            let file_threads: Vec<&(&ReviewThread, &PatchAnchor)> = on_patch
                .iter()
                .filter(|(_, anchor)| anchor.path.as_deref() == Some(file.path.as_str()))
                .collect();
            let is_collapsed = collapsed.contains(&key);
            rows.push(Row::File {
                patch: patch_index,
                file: Rc::clone(file),
                collapsed: is_collapsed,
                threads: file_threads.len(),
            });
            if is_collapsed {
                continue;
            }
            let mut file_rows = Vec::with_capacity(file.rows.len());
            for (index, row) in file.rows.iter().enumerate() {
                let line_key = row_line(row).map(|line| (file.path.clone(), row.t == "del", line));
                let commented = row.t != "hunk"
                    && line_key.as_ref().is_some_and(|key| covering.contains(key));
                file_rows.push(Row::Code {
                    patch: patch_index,
                    file: Rc::clone(file),
                    index,
                    commented,
                });
                if row.t == "hunk" {
                    continue;
                }
                let Some(line_key) = line_key else {
                    continue;
                };
                if let Some(ids) = ending.get(&line_key) {
                    for id in ids {
                        file_rows.push(Row::Thread {
                            thread_id: id.to_string(),
                        });
                        placed.insert(id);
                    }
                }
                for draft in drafts.iter().filter(|draft| {
                    draft.patch == patch_index
                        && end_of(&draft.anchor).as_ref() == Some(&line_key)
                }) {
                    file_rows.push(Row::Draft {
                        draft: Rc::clone(draft),
                    });
                }
            }
            // File threads, and line threads whose line is no longer in the diff, go under the file's header.
            for (thread, _) in file_threads {
                if !placed.contains(thread.id.as_str()) {
                    rows.push(Row::Thread {
                        thread_id: thread.id.clone(),
                    });
                    placed.insert(thread.id.as_str());
                }
            }
            for draft in drafts.iter().filter(|draft| {
                draft.patch == patch_index
                    && draft.anchor.anchor_type == PatchAnchorType::CodeFile
                    && draft.anchor.path.as_deref() == Some(file.path.as_str())
            }) {
                rows.push(Row::Draft {
                    draft: Rc::clone(draft),
                });
            }
            if file.rows.is_empty() {
                rows.push(Row::Note {
                    text: format!("{}: no lines to show ({}).", file.path, file.tag),
                });
            }
            rows.extend(file_rows);
        }
    }
    if loaded.patches.is_empty() {
        rows.push(Row::Note {
            text: "This review has no patch.".to_string(),
        });
    }
    rows
}

/// A request from elsewhere (the Review panel, Go to Comment) to show a thread on a review's pull-request page.
type Reveal = RwSignal<Option<(String, String)>>;

thread_local! {
    static REVEAL: RefCell<Option<Reveal>> = const { RefCell::new(None) };
}

fn reveal_signal() -> Reveal {
    REVEAL.with(|reveal| {
        *reveal
            .borrow_mut()
            .get_or_insert_with(|| Scope::new().create_rw_signal(None))
    })
}

/// Opens the review's pull-request page and scrolls to `thread_id` there (unfolding its file).
pub fn reveal_thread(window_tab: &WindowTabData, review_id: &str, thread_id: &str) {
    super::reveal_page(window_tab, ReviewPage::pull_request(review_id));
    reveal_signal().set(Some((review_id.to_string(), thread_id.to_string())));
}

/// Whether the review has patches to show as a pull request (reads its review directory: off the UI thread).
pub fn has_patches(review: &Review) -> bool {
    review.bundle.as_ref().is_some_and(|bundle| {
        !review_bundle::service::patches_of(Path::new(&bundle.dir)).is_empty()
    })
}

/// The width of a character of the editor font, for the column under the pointer.
fn char_width(config: &LapceConfig) -> f64 {
    let family: Vec<FamilyOwned> = FamilyOwned::parse_list(&config.editor.font_family).collect();
    let attrs = Attrs::new()
        .family(&family)
        .font_size(config.editor.font_size() as f32);
    let mut layout = TextLayout::new();
    layout.set_text("WWWWWWWWWW", AttrsList::new(attrs), None);
    (layout.size().width / 10.0).max(1.0)
}

/// Tabs as four spaces, for display; `column_of` maps a displayed column back to the line's.
fn display_text(text: &str) -> String {
    text.replace('\t', "    ")
}

fn column_of(text: &str, displayed: usize) -> usize {
    let mut shown = 0;
    for (column, character) in text.chars().enumerate() {
        let width = if character == '\t' { 4 } else { 1 };
        if displayed < shown + width {
            return column;
        }
        shown += width;
    }
    text.chars().count()
}

/// A word character at `column`, for hover and definition: only names have something to show.
fn is_word_at(text: &str, column: usize) -> bool {
    text.chars()
        .nth(column)
        .is_some_and(|character| character.is_alphanumeric() || character == '_')
}

#[derive(Clone)]
struct Hover {
    content: Vec<MarkdownContent>,
    at: Point,
}

/// The page's state, shared by its rows.
#[derive(Clone)]
struct Page {
    window_tab: Rc<WindowTabData>,
    review_id: String,
    review: Memo<Option<Arc<Review>>>,
    loaded: RwSignal<Option<Rc<Loaded>>>,
    collapsed: RwSignal<HashSet<FileKey>>,
    /// The review's drafts on the page's patches (from the window's drafts; not their text, so typing doesn't
    /// rebuild the rows).
    drafts: Memo<Vec<PlacedDraft>>,
    /// Each draft's suggested edit while it is being written, by draft id.
    suggestions: Rc<RefCell<HashMap<String, Suggestion>>>,
    scope: Scope,
    /// The line a shift-click finishes a range from: `(patch, path, side, line)`.
    range_start: RwSignal<Option<(usize, String, PatchSide, u32)>>,
    /// Measured heights of thread and draft rows (they size themselves), by key.
    heights: RwSignal<HashMap<RowKey, f64>>,
    hover: RwSignal<Option<Hover>>,
    hover_timer: RwSignal<TimerToken>,
    /// The pointer, in page coordinates (where a hover shows).
    pointer: RwSignal<Point>,
    char_width: Memo<f64>,
    config: ReadSignal<Arc<LapceConfig>>,
}

impl Page {
    fn patch(&self, index: usize) -> Option<(String, PathBuf)> {
        self.loaded.with_untracked(|loaded| {
            let patch = loaded.as_ref()?.patches.get(index)?;
            Some((patch.uri.clone(), patch.path.clone()))
        })
    }

    fn review_untracked(&self) -> Option<Arc<Review>> {
        thread::review_of(&self.window_tab, &self.review_id)
    }

    fn has_code(&self) -> bool {
        self.review
            .with(|review| review.as_ref().and_then(|review| code::code_folder(review)).is_some())
    }

    /// A draft on the patch (or the one already at `anchor`), kept with the window's drafts.
    fn add_draft(&self, patch: usize, anchor: PatchAnchor) {
        let Some((uri, _)) = self.patch(patch) else {
            return;
        };
        drafts::add_to(
            &self.window_tab,
            &self.review_id,
            draft_location(uri, anchor),
            ThreadIntent::Comment,
        );
    }

    fn discard_draft(&self, id: &str) {
        drafts::discard(&self.window_tab, id);
        self.suggestions.borrow_mut().remove(id);
    }

    /// The page's drafts, with their patch and suggestion.
    fn drafts_on(&self, loaded: &Loaded, placed: &[PlacedDraft]) -> Vec<Rc<Draft>> {
        let mut suggestions = self.suggestions.borrow_mut();
        placed
            .iter()
            .filter_map(|draft| {
                let patch = loaded.patches.iter().position(|patch| patch.uri == draft.uri)?;
                let suggestion = *suggestions
                    .entry(draft.id.clone())
                    .or_insert_with(|| self.scope.create_rw_signal(None));
                Some(Rc::new(Draft {
                    id: draft.id.clone(),
                    patch,
                    anchor: draft.anchor.clone(),
                    suggestion,
                }))
            })
            .collect()
    }

    /// Sends a draft as a new thread (off the UI thread); a suggestion goes with it as a pending proposal.
    fn submit_draft(&self, draft: &Draft, intent: ThreadIntent) {
        let Some((uri, _)) = self.patch(draft.patch) else {
            return;
        };
        let before = draft.anchor.source.clone().unwrap_or_default();
        let after = draft
            .suggestion
            .get_untracked()
            .map(|lines| lines.join("\n"))
            .filter(|after| *after != before);
        let Some(after) = after else {
            // A plain comment: the shared drafts send it (and bring it back if that fails).
            drafts::submit(&self.window_tab, &draft.id, intent);
            self.suggestions.borrow_mut().remove(&draft.id);
            return;
        };
        let body = self.window_tab.review.drafts.with_untracked(|drafts| {
            drafts
                .iter()
                .find(|shared| shared.id == draft.id)
                .map(|shared| shared.body.trim().to_string())
                .unwrap_or_default()
        });
        let body = if body.is_empty() {
            "Suggested edit.".to_string()
        } else {
            body
        };
        // A suggested edit is a comment with a pending proposal, whatever button sent it.
        let proposal = Some(Proposal {
            before,
            after,
            path: draft.anchor.path.clone(),
            start_line: draft.anchor.line.or(draft.anchor.start_line),
            status: ProposalStatus::Pending,
        });
        let location = draft_location(uri, draft.anchor.clone());
        let options = ThreadOptions {
            intent: Some(ThreadIntent::Comment),
            proposal,
            ..ThreadOptions::default()
        };
        let review_id = self.review_id.clone();
        thread::run(&self.window_tab, "commenting on the pull request", move |backend| {
            let (_, thread) = backend.services.store.create_thread(
                &review_id,
                location,
                body,
                backend.user(),
                options,
            )?;
            backend.ask_if_answering(&review_id, &thread);
            Ok(())
        });
        self.discard_draft(&draft.id);
    }

    /// The **+** on a diff line: a comment on it, or with shift a range from the last line commented on this side.
    fn comment_on_row(&self, patch: usize, file: &PatchFile, row: &PatchRow, shift: bool) {
        let Some(line) = row_line(row) else {
            return;
        };
        let side = row_side(row);
        let start = self.range_start.get_untracked();
        if shift
            && let Some((start_patch, start_path, start_side, start_line)) = start
            && start_patch == patch
            && start_path == file.path
            && start_side == side
            && start_line != line
        {
            let (first, last) = (start_line.min(line), start_line.max(line));
            let source = file
                .rows
                .iter()
                .filter(|other| other.t != "hunk" && row_side(other) == side)
                .filter(|other| row_line(other).is_some_and(|line| (first..=last).contains(&line)))
                .map(|other| other.s.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            self.add_draft(
                patch,
                PatchAnchor {
                    anchor_type: PatchAnchorType::CodeRange,
                    path: Some(file.path.clone()),
                    line: None,
                    start_line: Some(first),
                    end_line: Some(last),
                    side: Some(side),
                    source: Some(source),
                    extra: serde_json::Map::new(),
                },
            );
            self.range_start.set(None);
            return;
        }
        self.range_start
            .set(Some((patch, file.path.clone(), side, line)));
        self.add_draft(
            patch,
            PatchAnchor {
                anchor_type: PatchAnchorType::CodeLine,
                path: Some(file.path.clone()),
                line: Some(line),
                start_line: None,
                end_line: None,
                side: Some(side),
                source: Some(row.s.clone()),
                extra: serde_json::Map::new(),
            },
        );
    }

    /// A line number: new side opens the head file there, old side the base version (read-only).
    fn open_line(&self, path: &str, row: &PatchRow, side: PatchSide) {
        let Some(review) = self.review_untracked() else {
            return;
        };
        match (side, row.n, row.o) {
            (PatchSide::New, Some(line), _) => {
                code::open_head(&self.window_tab.main_split, &review, path, line, 0)
            }
            (PatchSide::Old, _, Some(line)) => {
                code::open_base(&self.window_tab, review, path.to_string(), line)
            }
            _ => {}
        }
    }

    fn hide_hover(&self) {
        self.hover_timer.set(TimerToken::INVALID);
        if self.hover.with_untracked(Option::is_some) {
            self.hover.set(None);
        }
    }
}

/// The pull-request page of a review.
pub fn pull_request_page(window_tab: Rc<WindowTabData>, review_id: String) -> impl View {
    let opened = Instant::now();
    let config = window_tab.common.config;
    let reviews = window_tab.review.reviews;
    let review = {
        let review_id = review_id.clone();
        create_memo(move |_| {
            reviews.with(|reviews| reviews.iter().find(|review| review.id == review_id).cloned())
        })
    };
    let page = Page {
        window_tab: window_tab.clone(),
        review_id: review_id.clone(),
        review,
        loaded: create_rw_signal(None),
        collapsed: create_rw_signal(HashSet::new()),
        drafts: {
            let (drafts, review_id) = (window_tab.review.drafts, review_id.clone());
            create_memo(move |_| drafts.with(|drafts| placed_drafts(drafts, &review_id)))
        },
        suggestions: Rc::default(),
        scope: Scope::current(),
        range_start: create_rw_signal(None),
        heights: create_rw_signal(HashMap::new()),
        hover: create_rw_signal(None),
        hover_timer: create_rw_signal(TimerToken::INVALID),
        pointer: create_rw_signal(Point::ZERO),
        char_width: create_memo(move |_| char_width(&config.get())),
        config,
    };
    load_when_changed(&page, opened);

    // Threads change often (every message); the rows only when a thread is added, removed or moves.
    let placements = create_memo(move |_| {
        review.with(|review| {
            review
                .as_ref()
                .map(|review| {
                    review
                        .threads
                        .iter()
                        .filter(|thread| thread.location.kind == LocationKind::Patch)
                        .map(|thread| {
                            let mut placed = thread.clone();
                            placed.messages.clear();
                            placed.updated_at = 0;
                            placed.agent_state = None;
                            placed
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    let rows = {
        let page = page.clone();
        create_memo(move |_| {
            let Some(loaded) = page.loaded.get() else {
                return Rc::new(Vec::new());
            };
            let started = Instant::now();
            let rows = placements.with(|threads| {
                page.collapsed.with(|collapsed| {
                    let drafts = page.drafts.with(|placed| page.drafts_on(&loaded, placed));
                    build_rows(&loaded, threads, collapsed, &drafts)
                })
            });
            if started.elapsed() > Duration::from_millis(50) {
                tracing::warn!(
                    "pull-request page: {} rows took {:?}",
                    rows.len(),
                    started.elapsed()
                );
            }
            Rc::new(rows)
        })
    };
    // Rows hold `Rc`s, which `Memo` can't compare cheaply: a new list is always a change.
    let rows = RowsSignal(rows);

    let header = header_view(page.clone());
    let list = list_view(page.clone(), rows, opened);
    let hover = hover_view(page.clone());
    let pointer = page.pointer;
    stack((
        stack((header, list)).style(|s| s.flex_col().size_full()),
        hover,
    ))
    .on_event_cont(EventListener::PointerMove, move |event| {
        if let Event::PointerMove(pointer_event) = event {
            pointer.set(pointer_event.pos);
        }
    })
    .style(move |s| {
        let config = config.get();
        s.size_full()
            .background(config.color(LapceColor::EDITOR_BACKGROUND))
            .color(config.color(LapceColor::EDITOR_FOREGROUND))
    })
    .debug_name("Pull Request Page")
}

#[derive(Clone, Copy)]
struct RowsSignal(Memo<Rc<Vec<Row>>>);

impl PartialEq for Row {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

/// Reads the review directory's patches off the UI thread when the page opens and whenever the review's patches may
/// have changed (a new head or base, or another directory).
fn load_when_changed(page: &Page, opened: Instant) {
    let page = page.clone();
    let cx = Scope::current();
    cx.create_effect(move |loaded_for: Option<Option<String>>| {
        let key = page.review.with(|review| {
            let review = review.as_ref()?;
            let bundle = review.bundle.as_ref()?;
            Some(format!(
                "{}\n{}\n{}",
                bundle.dir,
                bundle.head.as_deref().unwrap_or_default(),
                bundle.base.as_deref().unwrap_or_default()
            ))
        });
        if loaded_for.as_ref() == Some(&key) {
            return key;
        }
        let Some(dir) = page
            .review
            .with_untracked(|review| review.as_ref()?.bundle.as_ref().map(|bundle| bundle.dir.clone()))
        else {
            return key;
        };
        let (loaded, collapsed, review) = (page.loaded, page.collapsed, page.review);
        let send = floem::ext_event::create_ext_action(cx, move |parts: LoadedParts| {
            let read = opened.elapsed();
            let data = Loaded::from(parts);
            // Large files start folded, unless a thread is on them.
            let threaded: HashSet<String> = review.with_untracked(|review| {
                review
                    .as_ref()
                    .map(|review| {
                        review
                            .threads
                            .iter()
                            .filter_map(|thread| thread.location.patch_anchor.as_ref()?.path.clone())
                            .collect()
                    })
                    .unwrap_or_default()
            });
            let folded: HashSet<FileKey> = data
                .patches
                .iter()
                .enumerate()
                .flat_map(|(index, patch)| {
                    patch
                        .files
                        .iter()
                        .filter(|file| file.rows.len() > LARGE_FILE_ROWS && !threaded.contains(&file.path))
                        .map(move |file| (index, file.path.clone()))
                })
                .collect();
            tracing::info!(
                "pull-request page: {} files, {} diff lines read in {read:?}",
                data.patches.iter().map(|patch| patch.files.len()).sum::<usize>(),
                data.lines,
            );
            collapsed.set(folded);
            loaded.set(Some(Rc::new(data)));
        });
        let read = std::thread::Builder::new()
            .name("ReviewPullRequest".to_owned())
            .spawn(move || send(load(Path::new(&dir))));
        if let Err(error) = read {
            tracing::error!("could not read the pull request: {error}");
        }
        key
    });
}

/// Title, number, branch → base, commits, files and lines changed, the description, and the page's actions.
fn header_view(page: Page) -> impl View {
    let config = page.config;
    let review = page.review;
    let loaded = page.loaded;
    let title = move || {
        let meta_title = loaded.with(|loaded| {
            loaded
                .as_ref()
                .and_then(|loaded| loaded.meta.as_ref()?.title.clone())
        });
        meta_title
            .or_else(|| review.with(|review| review.as_ref().map(|review| review.title.clone())))
            .unwrap_or_else(|| "Pull request".to_string())
    };
    let number = move || {
        loaded
            .with(|loaded| {
                loaded
                    .as_ref()
                    .and_then(|loaded| loaded.meta.as_ref()?.number)
            })
            .map(|number| format!("#{number}"))
            .unwrap_or_default()
    };
    let summary = move || {
        loaded.with(|loaded| {
            let Some(loaded) = loaded.as_ref() else {
                return "Reading the patch…".to_string();
            };
            let files: Vec<&PatchFile> = loaded
                .patches
                .iter()
                .flat_map(|patch| patch.files.iter().map(Rc::as_ref))
                .collect();
            let (added, removed) = files
                .iter()
                .fold((0, 0), |(added, removed), file| (added + file.added, removed + file.removed));
            let mut parts = Vec::new();
            if let Some(meta) = &loaded.meta {
                if let Some(branch) = &meta.branch {
                    parts.push(match &meta.base {
                        Some(base) => format!("{branch} → {base}"),
                        None => branch.clone(),
                    });
                }
                if let Some(commits) = meta.commits {
                    parts.push(format!(
                        "{commits} commit{}",
                        if commits == 1 { "" } else { "s" }
                    ));
                }
            }
            parts.push(format!(
                "{} file{} changed +{added} −{removed}",
                files.len(),
                if files.len() == 1 { "" } else { "s" }
            ));
            parts.join(" · ")
        })
    };
    let body = {
        let window_tab = page.window_tab.clone();
        dyn_container(
            move || {
                loaded.with(|loaded| {
                    loaded
                        .as_ref()
                        .and_then(|loaded| loaded.meta.as_ref())
                        .map(|meta| meta.body.clone())
                        .filter(|body| !body.trim().is_empty())
                })
            },
            move |body| match body {
                Some(body) => scroll(markdown_body(window_tab.common.config, body))
                    .style(|s| s.width_full().max_height(220.0).margin_top(6.0))
                    .into_any(),
                None => empty().into_any(),
            },
        )
        .style(|s| s.width_full())
    };
    let actions = {
        let comment = {
            let page = page.clone();
            thread::text_button(
                "Comment on the pull request".to_string(),
                false,
                move || {
                    page.add_draft(
                        0,
                        PatchAnchor {
                            anchor_type: PatchAnchorType::Patch,
                            path: None,
                            line: None,
                            start_line: None,
                            end_line: None,
                            side: None,
                            source: None,
                            extra: serde_json::Map::new(),
                        },
                    )
                },
                config,
            )
        };
        let fold = {
            let page = page.clone();
            thread::text_button(
                "Fold all".to_string(),
                false,
                move || {
                    let all: HashSet<FileKey> = page.loaded.with_untracked(|loaded| {
                        loaded
                            .as_ref()
                            .map(|loaded| {
                                loaded
                                    .patches
                                    .iter()
                                    .enumerate()
                                    .flat_map(|(index, patch)| {
                                        patch.files.iter().map(move |file| (index, file.path.clone()))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default()
                    });
                    page.collapsed.set(all);
                },
                config,
            )
        };
        let unfold = {
            let collapsed = page.collapsed;
            thread::text_button(
                "Unfold all".to_string(),
                false,
                move || collapsed.set(HashSet::new()),
                config,
            )
        };
        stack((comment, fold, unfold)).style(|s| s.items_center())
    };
    stack((
        stack((
            label(title).style(|s| s.font_size(18.0).font_bold()),
            label(number).style(move |s| {
                s.margin_left(8.0)
                    .font_size(18.0)
                    .color(config.get().color(LapceColor::EDITOR_DIM))
            }),
            empty().style(|s| s.flex_grow(1.0_f32)),
            actions,
        ))
        .style(|s| s.width_full().items_center()),
        label(summary).style(move |s| {
            s.margin_top(4.0)
                .color(config.get().color(LapceColor::EDITOR_DIM))
        }),
        body,
    ))
    .style(move |s| {
        s.flex_col()
            .width_full()
            .padding_horiz(20.0)
            .padding_vert(12.0)
            .border_bottom(1.0)
            .border_color(config.get().color(LapceColor::LAPCE_BORDER))
    })
}

/// The scrolling, virtual list of rows.
fn list_view(page: Page, rows: RowsSignal, opened: Instant) -> impl View {
    let config = page.config;
    let heights = page.heights;
    let reveal_to = create_rw_signal(None::<Point>);
    // A thread asked for from elsewhere: unfold its file, then scroll to its row once the rows have it.
    {
        let page = page.clone();
        let reveal = reveal_signal();
        let review_id = page.review_id.clone();
        Scope::current().create_effect(move |_| {
            let Some((review, thread_id)) = reveal.get() else {
                return;
            };
            if review != review_id {
                return;
            }
            let path = page.review.with_untracked(|review| {
                review
                    .as_ref()?
                    .threads
                    .iter()
                    .find(|thread| thread.id == thread_id)?
                    .location
                    .patch_anchor
                    .as_ref()?
                    .path
                    .clone()
            });
            if let Some(path) = path
                && page.collapsed.with_untracked(|collapsed| {
                    collapsed.iter().any(|(_, folded)| *folded == path)
                })
            {
                page.collapsed
                    .update(|collapsed| collapsed.retain(|(_, folded)| *folded != path));
            }
            let target = RowKey::Thread(thread_id.clone());
            let line_height = config.get_untracked().editor.line_height() as f64;
            let offset = rows.0.with(|rows| {
                let mut offset = 0.0;
                for row in rows.iter() {
                    if row.key() == target {
                        return Some(offset);
                    }
                    offset += heights.with_untracked(|heights| row_height(row, heights, line_height));
                }
                None
            });
            if let Some(offset) = offset {
                reveal_to.set(Some(Point::new(0.0, (offset - 120.0).max(0.0))));
                reveal.set(None);
            }
        });
    }
    let first_layout = create_rw_signal(false);
    let view_page = page.clone();
    scroll(
        virtual_stack(
            move || Rows(rows.0.get()),
            Row::key,
            move |row| row_view(view_page.clone(), row),
        )
        .item_size_fn(move |row: &Row| {
            let line_height = config.get().editor.line_height() as f64;
            heights.with(|heights| row_height(row, heights, line_height))
        })
        .style(|s| s.flex_col().min_width_full().padding_bottom(40.0)),
    )
    .scroll_to(move || reveal_to.get())
    .on_resize(move |_| {
        if !first_layout.get_untracked() && rows.0.with_untracked(|rows| !rows.is_empty()) {
            first_layout.set(true);
            tracing::info!(
                "pull-request page: {} rows laid out {:?} after opening",
                rows.0.with_untracked(|rows| rows.len()),
                opened.elapsed()
            );
        }
    })
    .on_event_cont(EventListener::PointerWheel, {
        let page = page.clone();
        move |_| page.hide_hover()
    })
    .style(|s| s.width_full().flex_grow(1.0_f32).flex_basis(0.0_f32))
}

fn row_height(row: &Row, heights: &HashMap<RowKey, f64>, line_height: f64) -> f64 {
    match row {
        Row::Patch { .. } => 30.0,
        Row::File { .. } => 34.0,
        Row::Code { .. } => line_height,
        Row::Note { .. } => 28.0,
        Row::Thread { .. } => heights.get(&row.key()).copied().unwrap_or(110.0),
        Row::Draft { draft } => heights
            .get(&row.key())
            .copied()
            .unwrap_or(96.0 + draft.source_lines().len() as f64 * 30.0),
    }
}

fn row_view(page: Page, row: Row) -> impl View {
    let key = row.key();
    match row {
        Row::Patch { patch } => patch_view(page, patch).into_any(),
        Row::File {
            patch,
            file,
            collapsed,
            threads,
        } => file_header(page, patch, file, collapsed, threads).into_any(),
        Row::Code {
            patch,
            file,
            index,
            commented,
        } => code_row(page, patch, file, index, commented).into_any(),
        Row::Thread { thread_id } => measured(page.heights, key, {
            let review_id = page.review_id.clone();
            thread::thread_view(
                page.window_tab.clone(),
                review_id,
                thread_id,
                ThreadViewOptions::default(),
            )
        })
        .into_any(),
        Row::Draft { draft } => measured(page.heights, key, draft_view(page, draft)).into_any(),
        Row::Note { text } => {
            let config = page.config;
            label(move || text.clone())
                .style(move |s| {
                    s.height(28.0)
                        .padding_horiz(20.0)
                        .items_center()
                        .color(config.get().color(LapceColor::EDITOR_DIM))
                })
                .into_any()
        }
    }
}

/// A row that sizes itself: its height is recorded for the list's layout.
fn measured(heights: RwSignal<HashMap<RowKey, f64>>, key: RowKey, view: impl View + 'static) -> impl View {
    container(view)
        .on_resize(move |rect| {
            let height = rect.height();
            if height > 0.0
                && heights.with_untracked(|heights| heights.get(&key).is_none_or(|known| (known - height).abs() > 0.5))
            {
                heights.update(|heights| {
                    heights.insert(key.clone(), height);
                });
            }
        })
        .style(|s| {
            s.padding_horiz(20.0)
                .padding_vert(6.0)
                .width_full()
                .max_width(960.0)
                .min_width(480.0)
        })
}

fn patch_view(page: Page, patch: usize) -> impl View {
    let config = page.config;
    let name = page
        .loaded
        .with_untracked(|loaded| {
            loaded
                .as_ref()
                .and_then(|loaded| loaded.patches.get(patch).map(|patch| patch.name.clone()))
        })
        .unwrap_or_default();
    label(move || name.clone()).style(move |s| {
        s.height(30.0)
            .width_full()
            .padding_horiz(12.0)
            .items_center()
            .font_bold()
            .color(config.get().color(LapceColor::EDITOR_DIM))
    })
}

fn file_header(page: Page, patch: usize, file: Rc<PatchFile>, collapsed: bool, threads: usize) -> impl View {
    let config = page.config;
    let path = file.path.clone();
    let tag = file.tag.clone();
    let (added, removed) = (file.added, file.removed);
    let has_code = page.has_code();
    let toggle = {
        let page = page.clone();
        let key = (patch, path.clone());
        move || {
            page.collapsed.update(|collapsed| {
                if !collapsed.remove(&key) {
                    collapsed.insert(key.clone());
                }
            });
        }
    };
    let open = {
        let page = page.clone();
        let file = Rc::clone(&file);
        move || {
            let Some(review) = page.review_untracked() else {
                return;
            };
            let line = file.rows.iter().find_map(|row| row.n).unwrap_or(1);
            code::open_head(&page.window_tab.main_split, &review, &file.path, line, 0);
        }
    };
    let compare = {
        let page = page.clone();
        let file = Rc::clone(&file);
        move || {
            let Some(review) = page.review_untracked() else {
                return;
            };
            let line = file
                .rows
                .iter()
                .find(|row| row.t == "add")
                .and_then(|row| row.n);
            code::compare(&page.window_tab, review, file.path.clone(), line);
        }
    };
    let comment = {
        let page = page.clone();
        let path = path.clone();
        move || {
            page.add_draft(
                patch,
                PatchAnchor {
                    anchor_type: PatchAnchorType::CodeFile,
                    path: Some(path.clone()),
                    line: None,
                    start_line: None,
                    end_line: None,
                    side: None,
                    source: None,
                    extra: serde_json::Map::new(),
                },
            );
            page.collapsed
                .update(|collapsed| {
                    collapsed.remove(&(patch, path.clone()));
                });
        }
    };
    let deleted = tag == "deleted";
    let shown_tag = tag.clone();
    stack((
        label(move || if collapsed { "▸" } else { "▾" }.to_string())
            .style(|s| s.width(16.0).selectable(false)),
        label(move || path.clone()).style(|s| s.font_bold().selectable(false)),
        label(move || shown_tag.clone()).style(move |s| {
            let config = config.get();
            s.margin_left(8.0)
                .padding_horiz(6.0)
                .border(1.0)
                .border_radius(8.0)
                .font_size(11.0)
                .border_color(config.color(LapceColor::LAPCE_BORDER))
                .color(config.color(LapceColor::EDITOR_DIM))
                .apply_if(tag == "modified", |s| s.hide())
        }),
        label(move || format!("+{added}")).style(move |s| {
            s.margin_left(10.0)
                .color(config.get().color(LapceColor::SOURCE_CONTROL_ADDED))
        }),
        label(move || format!("−{removed}")).style(move |s| {
            s.margin_left(6.0)
                .color(config.get().color(LapceColor::SOURCE_CONTROL_REMOVED))
        }),
        label(move || format!("💬 {threads}")).style(move |s| {
            s.margin_left(10.0)
                .color(config.get().color(LapceColor::EDITOR_DIM))
                .apply_if(threads == 0, |s| s.hide())
        }),
        empty().style(|s| s.flex_grow(1.0_f32)),
        container(thread::text_button("Open file".to_string(), false, open, config))
            .style(move |s| s.apply_if(!has_code || deleted, |s| s.hide())),
        container(thread::text_button("Compare".to_string(), false, compare, config))
            .style(move |s| s.apply_if(!has_code || deleted, |s| s.hide())),
        thread::text_button("Comment".to_string(), false, comment, config),
    ))
    .on_click_stop(move |_| toggle())
    .style(move |s| {
        let config = config.get();
        s.height(34.0)
            .width_full()
            .min_width(720.0)
            .items_center()
            .padding_horiz(12.0)
            .margin_top(0.0)
            .cursor(CursorStyle::Pointer)
            .border_top(1.0)
            .border_bottom(1.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
            .background(config.color(LapceColor::PANEL_BACKGROUND))
    })
}

fn code_row(page: Page, patch: usize, file: Rc<PatchFile>, index: usize, commented: bool) -> impl View {
    let config = page.config;
    let Some(row) = file.rows.get(index).cloned() else {
        return empty().into_any();
    };
    let kind = row.t.clone();
    let has_code = page.has_code();
    let number = |line: Option<u32>, side: PatchSide| {
        let page = page.clone();
        let row = row.clone();
        let path = file.path.clone();
        let linked = has_code && line.is_some() && kind != "hunk";
        label(move || line.map(|line| line.to_string()).unwrap_or_default())
            .on_click_stop(move |_| {
                if linked {
                    page.open_line(&path, &row, side);
                }
            })
            .style(move |s| {
                let config = config.get();
                s.width(48.0)
                    .padding_right(8.0)
                    .justify_end()
                    .flex_shrink(0.0_f32)
                    .selectable(false)
                    .color(config.color(LapceColor::EDITOR_DIM))
                    .apply_if(linked, |s| {
                        s.cursor(CursorStyle::Pointer)
                            .hover(|s| s.color(config.color(LapceColor::EDITOR_LINK)))
                    })
            })
    };
    let old = number(row.o, PatchSide::Old);
    let new = number(row.n, PatchSide::New);
    let plus = {
        let page = page.clone();
        let file = Rc::clone(&file);
        let row = row.clone();
        let is_hunk = kind == "hunk";
        label(|| "+".to_string())
            .on_event_stop(EventListener::PointerDown, move |event| {
                if let Event::PointerDown(pointer) = event
                    && !is_hunk
                {
                    page.comment_on_row(patch, &file, &row, pointer.modifiers.shift());
                }
            })
            .style(move |s| {
                let config = config.get();
                s.width(18.0)
                    .flex_shrink(0.0_f32)
                    .justify_center()
                    .selectable(false)
                    .cursor(CursorStyle::Pointer)
                    .color(config.color(LapceColor::EDITOR_DIM).multiply_alpha(0.5))
                    .hover(|s| {
                        s.color(config.color(LapceColor::EDITOR_LINK))
                            .background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
                    })
                    .apply_if(is_hunk, |s| s.hide())
            })
    };
    let sign = match kind.as_str() {
        "add" => "+",
        "del" => "−",
        "hunk" => "",
        _ => " ",
    };
    let shown = format!("{sign}{}", display_text(&row.s));
    let text = {
        let page = page.clone();
        let hover_page = page.clone();
        let leave_page = page.clone();
        let source = row.s.clone();
        let hover_source = row.s.clone();
        let path = file.path.clone();
        let hover_path = file.path.clone();
        // New-side lines are in the head file, where the language server knows the names.
        let line = row.n.filter(|_| has_code && (kind == "add" || kind == "ctx"));
        let char_width = page.char_width;
        let column_at = move |x: f64, source: &str| {
            let displayed = (x / char_width.get_untracked()).floor().max(0.0) as usize;
            // The first character shown is the sign.
            displayed.checked_sub(1).map(|displayed| column_of(source, displayed))
        };
        label(move || shown.clone())
            .on_event_cont(EventListener::PointerDown, move |event| {
                let Event::PointerDown(pointer) = event else {
                    return;
                };
                let command = if cfg!(target_os = "macos") {
                    pointer.modifiers.meta()
                } else {
                    pointer.modifiers.control()
                };
                let (Some(line), true) = (line, command) else {
                    return;
                };
                let Some(column) = column_at(pointer.pos.x, &source) else {
                    return;
                };
                if let Some(review) = page.review_untracked() {
                    page.hide_hover();
                    code::go_to_definition(&page.window_tab, review, path.clone(), line, column as u32);
                }
            })
            .on_event_cont(EventListener::PointerMove, move |event| {
                let Event::PointerMove(pointer) = event else {
                    return;
                };
                let page = hover_page.clone();
                let Some(line) = line else {
                    return;
                };
                let column = column_at(pointer.pos.x, &hover_source)
                    .filter(|column| is_word_at(&hover_source, *column));
                let Some(column) = column else {
                    page.hide_hover();
                    return;
                };
                if page.hover.with_untracked(Option::is_some) {
                    page.hover.set(None);
                }
                let path = hover_path.clone();
                let timer = page.hover_timer;
                let token = exec_after(Duration::from_millis(600), move |token| {
                    if timer.try_get_untracked() != Some(token) {
                        return;
                    }
                    let Some(review) = page.review_untracked() else {
                        return;
                    };
                    let at = page.pointer.get_untracked();
                    let hover = page.hover;
                    code::hover(&page.window_tab, &review, &path, line, column as u32, move |content| {
                        if timer.get_untracked() == token {
                            hover.set(Some(Hover {
                                content,
                                at,
                            }));
                        }
                    });
                });
                timer.set(token);
            })
            .on_event_cont(EventListener::PointerLeave, move |_| leave_page.hide_hover())
            .style(move |s| {
                let config = config.get();
                s.font_family(config.editor.font_family.clone())
                    .font_size(config.editor.font_size() as f32)
                    .padding_right(20.0)
                    .apply_if(kind == "hunk", |s| s.color(config.color(LapceColor::EDITOR_DIM)))
            })
    };
    let background_kind = row.t.clone();
    stack((old, new, plus, text))
        .style(move |s| {
            let config = config.get();
            let line_height = config.editor.line_height() as f32;
            let background = match background_kind.as_str() {
                "add" => Some(config.color(LapceColor::SOURCE_CONTROL_ADDED).multiply_alpha(0.15)),
                "del" => Some(config.color(LapceColor::SOURCE_CONTROL_REMOVED).multiply_alpha(0.15)),
                "hunk" => Some(config.color(LapceColor::PANEL_BACKGROUND)),
                _ => None,
            };
            s.height(line_height)
                .min_width_full()
                .items_center()
                .font_family(config.editor.font_family.clone())
                .font_size(config.editor.font_size() as f32)
                .apply_opt(background, |s, background| s.background(background))
                .apply_if(commented, |s| {
                    s.border_left(3.0)
                        .border_color(config.color(LapceColor::EDITOR_LINK))
                })
        })
        .into_any()
}

/// A comment being written: what it is on, its text, and (on lines) a suggested edit, one input per line.
fn draft_view(page: Page, draft: Rc<Draft>) -> impl View {
    let config = page.config;
    let describe = describe(&draft.anchor);
    let submit = {
        let page = page.clone();
        let draft = Rc::clone(&draft);
        move |intent: ThreadIntent| page.submit_draft(&draft, intent)
    };
    let body = {
        let submit = submit.clone();
        let initial = page.window_tab.review.drafts.with_untracked(|drafts| {
            drafts
                .iter()
                .find(|shared| shared.id == draft.id)
                .map(|shared| shared.body.clone())
                .unwrap_or_default()
        });
        let (submit_tab, change_tab) = (page.window_tab.clone(), page.window_tab.clone());
        let (submit_id, change_id) = (draft.id.clone(), draft.id.clone());
        thread::reply_box(
            page.window_tab.clone(),
            "Leave a comment… (Enter to send)",
            initial,
            move |text| {
                drafts::set_body(&submit_tab, &submit_id, text);
                submit(ThreadIntent::Comment);
            },
            move |text| drafts::set_body(&change_tab, &change_id, text),
        )
    };
    let suggestion = {
        let window_tab = page.window_tab.clone();
        let draft = Rc::clone(&draft);
        let submit = submit.clone();
        let suggestion = draft.suggestion;
        dyn_container(
            move || suggestion.with(Option::is_some),
            move |suggesting| {
                if !suggesting {
                    return empty().into_any();
                }
                let lines = draft.suggestion.get_untracked().unwrap_or_default();
                let suggestion = draft.suggestion;
                v_stack_from_iter(lines.into_iter().enumerate().map(|(index, line)| {
                    let submit = submit.clone();
                    thread::reply_box(
                        window_tab.clone(),
                        "(empty line)",
                        line,
                        move |_| submit(ThreadIntent::Comment),
                        move |text| {
                            suggestion.update(|lines| {
                                if let Some(line) = lines.as_mut().and_then(|lines| lines.get_mut(index)) {
                                    *line = text;
                                }
                            })
                        },
                    )
                    .style(move |s| {
                        s.margin_top(2.0)
                            .font_family(config.get().editor.font_family.clone())
                            .background(config.get().color(LapceColor::SOURCE_CONTROL_ADDED).multiply_alpha(0.1))
                    })
                }))
                .style(|s| s.flex_col().width_full().margin_top(4.0))
                .into_any()
            },
        )
        .style(|s| s.width_full())
    };
    let can_suggest = draft.can_suggest();
    let toggle_suggestion = {
        let draft = Rc::clone(&draft);
        move || {
            let lines = draft.source_lines();
            draft.suggestion.update(|suggestion| {
                *suggestion = match suggestion {
                    Some(_) => None,
                    None => Some(lines.clone()),
                }
            });
        }
    };
    let suggesting = draft.suggestion;
    let buttons = {
        let cancel = {
            let page = page.clone();
            let id = draft.id.clone();
            move || page.discard_draft(&id)
        };
        let ask = {
            let submit = submit.clone();
            move || submit(ThreadIntent::Question)
        };
        let comment = move || submit(ThreadIntent::Comment);
        stack((
            container(thread::text_button(
                "Suggest a change".to_string(),
                false,
                toggle_suggestion,
                config,
            ))
            .style(move |s| s.apply_if(!can_suggest, |s| s.hide())),
            empty().style(|s| s.flex_grow(1.0_f32)),
            thread::text_button("Cancel".to_string(), false, cancel, config),
            container(thread::text_button("Ask agent".to_string(), false, ask, config))
                .style(move |s| s.apply_if(suggesting.with(Option::is_some), |s| s.hide())),
            thread::text_button("Comment".to_string(), true, comment, config),
        ))
        .style(|s| s.width_full().items_center().margin_top(6.0))
    };
    stack((
        label(move || describe.clone()).style(move |s| {
            s.margin_bottom(4.0)
                .color(config.get().color(LapceColor::EDITOR_DIM))
        }),
        body,
        suggestion,
        buttons,
    ))
    .style(move |s| {
        let config = config.get();
        s.flex_col()
            .width_full()
            .padding(8.0)
            .border(1.0)
            .border_radius(6.0)
            .border_color(config.color(LapceColor::EDITOR_LINK))
            .background(config.color(LapceColor::PANEL_BACKGROUND))
    })
}

/// What a patch anchor is on, as the draft's label says it.
fn describe(anchor: &PatchAnchor) -> String {
    let side = match anchor.side {
        Some(PatchSide::Old) => " (old)",
        _ => "",
    };
    let path = anchor.path.as_deref().unwrap_or_default();
    match anchor.anchor_type {
        PatchAnchorType::Patch => "Comment on the pull request".to_string(),
        PatchAnchorType::CodeFile => format!("Comment on {path}"),
        PatchAnchorType::CodeLine => format!(
            "Comment on {path}:{}{side}",
            anchor.line.unwrap_or_default()
        ),
        PatchAnchorType::CodeRange => format!(
            "Comment on {path}:{}–{}{side}",
            anchor.start_line.unwrap_or_default(),
            anchor.end_line.unwrap_or_default()
        ),
        PatchAnchorType::Other => "Comment".to_string(),
    }
}

/// Rendered Markdown (Lapce's renderer), for the pull request's description.
fn markdown_body(config: ReadSignal<Arc<LapceConfig>>, body: String) -> impl View {
    let contents = crate::markdown::parse_markdown(&body, 1.5, &config.get_untracked());
    markdown_contents(config, contents)
}

fn markdown_contents(config: ReadSignal<Arc<LapceConfig>>, contents: Vec<MarkdownContent>) -> impl View {
    v_stack_from_iter(contents.into_iter().map(move |content| match content {
        MarkdownContent::Text(layout) => rich_text(move || layout.clone())
            .style(|s| s.max_width_pct(100.0))
            .into_any(),
        MarkdownContent::Image { .. } => empty().into_any(),
        MarkdownContent::Separator => empty()
            .style(move |s| {
                s.width_full()
                    .margin_vert(5.0)
                    .height(1.0)
                    .background(config.get().color(LapceColor::LAPCE_BORDER))
            })
            .into_any(),
    }))
    .style(|s| s.flex_col().width_full())
}

/// The language server's hover for the name under the pointer.
fn hover_view(page: Page) -> impl View {
    let config = page.config;
    let hover = page.hover;
    dyn_container(
        move || hover.get().map(|hover| (hover.at, hover.content)),
        move |shown| {
            let Some((at, content)) = shown else {
                return empty().into_any();
            };
            v_stack_from_iter(content.into_iter().map(move |content| match content {
                MarkdownContent::Text(layout) => rich_text(move || layout.clone())
                    .style(|s| s.max_width(600.0))
                    .into_any(),
                MarkdownContent::Image { .. } => empty().into_any(),
                MarkdownContent::Separator => empty()
                    .style(move |s| {
                        s.width_full()
                            .margin_vert(5.0)
                            .height(1.0)
                            .background(config.get().color(LapceColor::LAPCE_BORDER))
                    })
                    .into_any(),
            }))
            .style(move |s| {
                let config = config.get();
                s.flex_col()
                    .absolute()
                    .inset_left(at.x as f32 + 8.0)
                    .inset_top(at.y as f32 + 16.0)
                    .max_width(620.0)
                    .padding_horiz(10.0)
                    .padding_vert(6.0)
                    .border(1.0)
                    .border_radius(6.0)
                    .border_color(config.color(LapceColor::LAPCE_BORDER))
                    .background(config.color(LapceColor::PANEL_BACKGROUND))
                    .z_index(10)
            })
            .into_any()
        },
    )
    .style(|s| s.absolute().size_full().pointer_events_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(text: &str) -> Loaded {
        Loaded::from(LoadedParts {
            meta: None,
            patches: vec![(
                PathBuf::from("/review/change.patch"),
                "file:///review/change.patch".to_string(),
                "change.patch".to_string(),
                parse_patch(text),
            )],
        })
    }

    // The patch's last line break leaves an empty context row, as the TypeScript parser does.
    const PATCH: &str = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,3 @@\n fn a() {\n-    old();\n+    new();\n }\n";

    fn thread_on(anchor: PatchAnchor) -> ReviewThread {
        serde_json::from_value(serde_json::json!({
            "id": "t1",
            "number": 1,
            "location": {
                "kind": "patch",
                "uri": "file:///review/change.patch",
                "patchAnchor": serde_json::to_value(anchor).unwrap(),
            },
            "messages": [],
            "status": "open",
            "intent": "comment",
            "createdAt": 0,
            "updatedAt": 0,
        }))
        .unwrap()
    }

    fn line_anchor(line: u32, side: PatchSide) -> PatchAnchor {
        PatchAnchor {
            anchor_type: PatchAnchorType::CodeLine,
            path: Some("src/a.rs".to_string()),
            line: Some(line),
            start_line: None,
            end_line: None,
            side: Some(side),
            source: None,
            extra: serde_json::Map::new(),
        }
    }

    fn keys(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row.key() {
                RowKey::Patch(_) => "patch".to_string(),
                RowKey::File(_, path, ..) => format!("file {path}"),
                RowKey::Code(_, _, index, commented) => {
                    format!("code {index}{}", if commented { "*" } else { "" })
                }
                RowKey::Thread(id) => format!("thread {id}"),
                RowKey::Draft(id) => format!("draft {id}"),
                RowKey::Note(text) => format!("note {text}"),
            })
            .collect()
    }

    #[test]
    fn the_pages_drafts_are_the_reviews_shared_drafts_on_a_patch() {
        let uri = "file:///review/change.patch".to_string();
        let shared = |id: &str, review_id: &str, location: CodeLocation| ReviewDraft {
            id: id.to_string(),
            review_id: review_id.to_string(),
            location,
            body: "kept".to_string(),
            intent: ThreadIntent::Comment,
        };
        let mut on_file = draft_location(uri.clone(), line_anchor(2, PatchSide::New));
        on_file.kind = LocationKind::File;
        let drafts: im::Vector<ReviewDraft> = [
            shared("d1", "r1", draft_location(uri.clone(), line_anchor(2, PatchSide::Old))),
            shared("d2", "r2", draft_location(uri.clone(), line_anchor(2, PatchSide::Old))),
            shared("d3", "r1", on_file),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            placed_drafts(&drafts, "r1"),
            vec![PlacedDraft {
                id: "d1".to_string(),
                uri,
                anchor: line_anchor(2, PatchSide::Old),
            }]
        );
    }

    #[test]
    fn an_old_side_thread_follows_its_removed_line() {
        let loaded = patch(PATCH);
        let threads = vec![thread_on(line_anchor(2, PatchSide::Old))];
        let rows = build_rows(&loaded, &threads, &HashSet::new(), &[]);
        assert_eq!(
            keys(&rows),
            ["file src/a.rs", "code 0", "code 1", "code 2*", "thread t1", "code 3", "code 4", "code 5"]
        );
    }

    #[test]
    fn a_new_side_thread_follows_its_added_line() {
        let loaded = patch(PATCH);
        let threads = vec![thread_on(line_anchor(2, PatchSide::New))];
        let rows = build_rows(&loaded, &threads, &HashSet::new(), &[]);
        assert_eq!(
            keys(&rows),
            ["file src/a.rs", "code 0", "code 1", "code 2", "code 3*", "thread t1", "code 4", "code 5"]
        );
    }

    #[test]
    fn a_folded_file_shows_its_header_only() {
        let loaded = patch(PATCH);
        let folded = HashSet::from([(0, "src/a.rs".to_string())]);
        let rows = build_rows(&loaded, &[], &folded, &[]);
        assert_eq!(keys(&rows), ["file src/a.rs"]);
    }

    #[test]
    fn a_thread_whose_line_left_the_diff_stays_under_its_file() {
        let loaded = patch(PATCH);
        let threads = vec![thread_on(line_anchor(40, PatchSide::New))];
        let rows = build_rows(&loaded, &threads, &HashSet::new(), &[]);
        assert_eq!(keys(&rows)[1], "thread t1");
    }

    #[test]
    fn columns_map_through_tabs() {
        assert_eq!(column_of("\tab", 0), 0);
        assert_eq!(column_of("\tab", 3), 0);
        assert_eq!(column_of("\tab", 4), 1);
        assert_eq!(column_of("\tab", 5), 2);
    }

    #[test]
    fn five_thousand_lines_build_quickly() {
        let mut text = String::new();
        for file in 0..50 {
            text.push_str(&format!(
                "diff --git a/f{file}.rs b/f{file}.rs\n--- a/f{file}.rs\n+++ b/f{file}.rs\n@@ -1,50 +1,50 @@\n"
            ));
            for line in 0..50 {
                text.push_str(&format!("-old {line}\n+new {line}\n"));
            }
        }
        let started = Instant::now();
        let loaded = patch(&text);
        let rows = build_rows(&loaded, &[], &HashSet::new(), &[]);
        assert!(rows.len() >= 5_000);
        assert!(started.elapsed() < Duration::from_millis(300), "{:?}", started.elapsed());
    }
}
