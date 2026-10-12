//! Review directories: a document (`index.markdown`, `*.pseudocode.md`, …), `*.patch` / `*.diff`
//! pages, `PR.md` metadata and `<patch>.comments.json[l]` pre-seeded findings. The review state is
//! also written to the directory as `review.json`. Port of `extensions/review/src/node/bundle-service.ts`.
//!
//! Executor-agnostic: watching and exporting run on std threads.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, Result, anyhow};
use regex::Regex;
use review_anchor::Edit;
use review_model::{
    LocationKind, Participant, ParticipantKind, Proposal, ProposalStatus, Review, ReviewBundle,
    ReviewScope, Severity, ThreadOptions, ThreadStatus, content_version,
};
use review_store::{
    CreateReviewParams, ReviewChange, ReviewStore, co_review_home, path_to_file_uri, real_path,
};
use sha2::{Digest as _, Sha256};

use crate::findings::{self, Finding};
use crate::patch::{DOCUMENT_NAMES, parse_patch_meta};
use crate::payloads::{
    StateDocument, file_uri_to_path, location_from_anchor, patch_slug, resolve_path, to_state_file,
};
use crate::tracking::{self, AppliedEdit};
use review_store::snapshots::index_of;

/// How long findings files must be quiet before they are synced again.
const SYNC_DEBOUNCE: Duration = Duration::from_millis(300);
/// How often a watched directory's findings files are checked.
const WATCH_INTERVAL: Duration = Duration::from_millis(100);
/// How long a review must be quiet before its `review.json` is written.
const EXPORT_DEBOUNCE: Duration = Duration::from_millis(150);

/// What to open: a review directory, or inline content written to one first.
#[derive(Debug, Clone)]
pub struct OpenBundleParams {
    pub dir: Option<PathBuf>,
    pub markdown: Option<String>,
    pub patch: Option<String>,
    pub patch_name: Option<String>,
    pub title: Option<String>,
    pub store_path: Option<PathBuf>,
    /// OpenSpec change directory (…/changes/<id>); `<dir>/openspec` is picked up automatically.
    pub openspec: Option<PathBuf>,
    pub author: Participant,
}

impl OpenBundleParams {
    pub fn dir(dir: impl Into<PathBuf>, author: Participant) -> Self {
        Self {
            dir: Some(dir.into()),
            markdown: None,
            patch: None,
            patch_name: None,
            title: None,
            store_path: None,
            openspec: None,
            author,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OpenedBundle {
    pub review: Review,
    pub dir: PathBuf,
    pub doc_file: Option<PathBuf>,
    pub patches: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct BundleServiceOptions {
    /// Where inline content is written (`<home>/inline/<hash>/`); default `co_review_home()`.
    pub home: PathBuf,
    /// Write `review.json` whenever a review directory's review changes (debounced).
    pub export_on_change: bool,
}

impl Default for BundleServiceOptions {
    fn default() -> Self {
        Self {
            home: co_review_home(),
            export_on_change: true,
        }
    }
}

pub struct BundleService {
    shared: Arc<Shared>,
}

struct Shared {
    store: Arc<ReviewStore>,
    home: PathBuf,
    watchers: Mutex<HashMap<PathBuf, Watcher>>,
    /// One export at a time: exports of a review share a temporary file.
    exporting: Mutex<()>,
    /// One anchor resolution at a time, so two callers don't both re-base a review.
    resolving: Mutex<()>,
}

struct Watcher {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take()
            && thread.thread().id() != thread::current().id()
        {
            thread.join().ok();
        }
    }
}

impl BundleService {
    pub fn new(store: Arc<ReviewStore>) -> Self {
        Self::with_options(store, BundleServiceOptions::default())
    }

    pub fn with_options(store: Arc<ReviewStore>, options: BundleServiceOptions) -> Self {
        let shared = Arc::new(Shared {
            store,
            home: options.home,
            watchers: Mutex::new(HashMap::new()),
            exporting: Mutex::new(()),
            resolving: Mutex::new(()),
        });
        if options.export_on_change {
            spawn_exporter(&shared);
        }
        Self { shared }
    }

    pub fn store(&self) -> &Arc<ReviewStore> {
        &self.shared.store
    }

    /// Opens (or re-opens) the review of a review directory; inline content is written to a
    /// directory first.
    pub fn open(&self, params: OpenBundleParams) -> Result<OpenedBundle> {
        let shared = &self.shared;
        let dir = match &params.dir {
            Some(dir) => dir.clone(),
            None => self.write_inline(&params)?,
        };
        let dir = real_path(&resolve_path(&dir));
        let dir_text = dir.to_string_lossy().into_owned();
        let workspace_root = path_to_file_uri(&dir_text);
        let doc_file = document_of(&dir);
        let title = match (&params.title, &doc_file) {
            (Some(title), _) => title.clone(),
            (None, Some(doc_file)) => title_of(doc_file)?,
            (None, None) => file_name(&dir),
        };
        let review = match shared.store.list(&workspace_root).into_iter().next() {
            Some(review) => review,
            None => shared.store.create(CreateReviewParams {
                workspace_root,
                title,
                scope: ReviewScope::Repository,
                author: params.author.clone(),
            })?,
        };
        let detected = dir
            .join("openspec")
            .join("proposal.md")
            .exists()
            .then(|| dir.join("openspec").to_string_lossy().into_owned());
        let previous = review.bundle.clone();
        let openspec = params
            .openspec
            .as_deref()
            .map(|openspec| resolve_path(openspec).to_string_lossy().into_owned())
            .or_else(|| previous.as_ref().and_then(|bundle| bundle.openspec.clone()))
            .or(detected);
        let store_path = params
            .store_path
            .as_deref()
            .map(|store_path| resolve_path(store_path).to_string_lossy().into_owned())
            .or_else(|| {
                previous
                    .as_ref()
                    .and_then(|bundle| bundle.store_path.clone())
            });
        let bundle = match previous {
            Some(bundle) => ReviewBundle {
                dir: dir_text,
                store_path,
                openspec,
                ..bundle
            },
            None => ReviewBundle {
                dir: dir_text,
                store_path,
                openspec,
                doc_version: None,
                github: None,
                code: None,
                repo: None,
                base: None,
                head: None,
                worktree: None,
                checkout: None,
                related: None,
                extra: serde_json::Map::new(),
            },
        };
        let review = shared.store.set_bundle(&review.id, Some(bundle))?;
        shared.sync_findings(&review.id)?;
        Shared::watch(shared, &review.id, &dir);
        let review = shared
            .store
            .get(&review.id)
            .ok_or_else(|| anyhow!("review {} disappeared while opening it", review.id))?;
        Ok(OpenedBundle {
            review,
            patches: patches_of(&dir),
            doc_file,
            dir,
        })
    }

    /// Writes inline content to `<home>/inline/<hash>/`: the same content, the same directory
    /// (and so the same review).
    pub fn write_inline(&self, params: &OpenBundleParams) -> Result<PathBuf> {
        let markdown = params.markdown.as_deref().unwrap_or("");
        let patch = params.patch.as_deref().unwrap_or("");
        let hash = format!(
            "{:x}",
            Sha256::digest(format!("{markdown}\0{patch}").as_bytes())
        );
        let dir = self.shared.home.join("inline").join(&hash[..12]);
        fs::create_dir_all(&dir)
            .with_context(|| format!("creating inline directory {}", dir.display()))?;
        if !markdown.is_empty() {
            static HEADING: LazyLock<Option<Regex>> =
                LazyLock::new(|| Regex::new(r"(?m)^#\s+").ok());
            let has_heading = HEADING
                .as_ref()
                .is_some_and(|heading| heading.is_match(markdown));
            let text = match &params.title {
                Some(title) if !title.is_empty() && !has_heading => {
                    format!("# {title}\n\n{markdown}")
                }
                _ => markdown.to_string(),
            };
            fs::write(dir.join("index.markdown"), text)?;
        }
        if !patch.is_empty() {
            let name = params
                .patch_name
                .as_deref()
                .filter(|name| !name.is_empty())
                .unwrap_or("change");
            fs::write(dir.join(format!("{name}.patch")), patch)?;
        }
        Ok(dir)
    }

    /// Accepting a patch suggestion records it (the agent applies it as a commit; the reviewed
    /// patch stays as it is); accepting a document suggestion writes it into the document.
    pub fn decide_proposal(&self, review_id: &str, thread_id: &str, accept: bool) -> Result<()> {
        self.shared.decide_proposal(review_id, thread_id, accept)
    }

    /// Pre-seeded findings from `<patch>.comments.json` / `.jsonl`, as proposed threads;
    /// idempotent by finding id. Returns how many threads were created.
    pub fn sync_findings(&self, review_id: &str) -> Result<usize> {
        self.shared.sync_findings(review_id)
    }

    pub fn add_finding(
        &self,
        review_id: &str,
        dir: &Path,
        doc_file: Option<&Path>,
        finding: &Finding,
        author: Option<Participant>,
    ) -> Result<()> {
        self.shared
            .add_finding(review_id, dir, doc_file, finding, author)
    }

    /// Writes the review's `review.json` (or the configured store path).
    pub fn export(&self, review_id: &str) -> Result<()> {
        self.shared.export(review_id)
    }

    /// Brings a review's tracked anchors up to date with its files.
    pub fn resolve_anchors(&self, review_id: &str) -> Result<()> {
        self.shared.resolve(review_id, None)
    }

    /// Stops ingesting findings files of `dir` as they change.
    pub fn unwatch(&self, dir: &Path) {
        self.shared.unwatch(dir);
    }

    pub fn unwatch_all(&self) {
        let watchers: Vec<Watcher> = match self.shared.watchers.lock() {
            Ok(mut watchers) => watchers.drain().map(|(_, watcher)| watcher).collect(),
            Err(_) => return,
        };
        for watcher in watchers {
            watcher.stop();
        }
    }
}

impl Drop for BundleService {
    fn drop(&mut self) {
        self.unwatch_all();
    }
}

impl Shared {
    fn decide_proposal(&self, review_id: &str, thread_id: &str, accept: bool) -> Result<()> {
        let Some(review) = self.store.get(review_id) else {
            return Ok(());
        };
        let Some(thread) = review.threads.iter().find(|thread| thread.id == thread_id) else {
            return Ok(());
        };
        let Some(proposal) = thread
            .proposal
            .as_ref()
            .filter(|proposal| proposal.status == ProposalStatus::Pending)
        else {
            return Ok(());
        };
        if !accept {
            self.store.decide_proposal(
                review_id,
                thread_id,
                false,
                "Edit rejected.".into(),
                None,
            )?;
            return Ok(());
        }
        let uri = thread.location.uri.as_deref();
        if thread.location.kind == LocationKind::Patch
            && let Some(patch) = uri.and_then(file_uri_to_path)
        {
            let branch = branch_of_patch(&patch);
            self.store.decide_proposal(
                review_id,
                thread_id,
                true,
                format!(
                    "Suggestion accepted. Apply it as a new commit on {}; the reviewed patch stays as it is.",
                    branch.as_deref().unwrap_or("the branch")
                ),
                None,
            )?;
            return Ok(());
        }
        let file = uri.and_then(file_uri_to_path);
        let text = file
            .as_ref()
            .filter(|file| file.exists())
            .map(fs::read_to_string)
            .transpose()?;
        let placed = file.zip(text).zip(uri).and_then(|((file, text), uri)| {
            let units: Vec<u16> = text.encode_utf16().collect();
            let before: Vec<u16> = proposal.before.encode_utf16().collect();
            // Where the comment is (its tracked place), not the first place the same words appear.
            let tracked_from = thread.location.tracked.as_ref().and_then(|tracked| {
                let from = usize::try_from(tracked.from).ok()?;
                (tracked.version == content_version(&text)
                    && units.get(from..from + before.len()) == Some(before.as_slice()))
                .then_some(from)
            });
            let from = tracked_from.or_else(|| index_of(&units, &before, 0))?;
            Some((file, text, units, uri, from, before.len()))
        });
        let Some((file, text, units, uri, from, length)) = placed else {
            self.store.decide_proposal(
                review_id,
                thread_id,
                true,
                "Accepted, but the quoted text was no longer in the source, so no new version was written.".into(),
                None,
            )?;
            return Ok(());
        };
        let edit = Edit {
            from,
            to: from + length,
            insert: proposal.after.clone(),
        };
        let mut edited: Vec<u16> = units[..edit.from].to_vec();
        edited.extend(edit.insert.encode_utf16());
        edited.extend_from_slice(&units[edit.to..]);
        fs::write(&file, String::from_utf16_lossy(&edited))
            .with_context(|| format!("writing the accepted edit to {}", file.display()))?;
        // The document's other comments follow this edit exactly.
        self.resolve(
            review_id,
            Some(&AppliedEdit {
                uri,
                before: &text,
                edits: std::slice::from_ref(&edit),
            }),
        )?;
        let version = review
            .bundle
            .as_ref()
            .and_then(|bundle| bundle.doc_version)
            .unwrap_or(1)
            + 1;
        self.store.decide_proposal(
            review_id,
            thread_id,
            true,
            format!("Edit accepted. Document version {version} created."),
            Some(version),
        )?;
        Ok(())
    }

    fn resolve(&self, review_id: &str, applied: Option<&AppliedEdit>) -> Result<()> {
        let _resolving = self
            .resolving
            .lock()
            .map_err(|_| anyhow!("anchor resolution lock poisoned"))?;
        tracking::resolve(&self.store, review_id, applied)
    }

    fn sync_findings(&self, review_id: &str) -> Result<usize> {
        let Some(review) = self.store.get(review_id) else {
            return Ok(0);
        };
        let Some(dir) = review
            .bundle
            .as_ref()
            .map(|bundle| PathBuf::from(&bundle.dir))
            .filter(|dir| dir.exists())
        else {
            return Ok(0);
        };
        let mut seen: HashSet<String> = review
            .threads
            .iter()
            .filter_map(|thread| thread.source_id.clone())
            .filter(|source_id| !source_id.is_empty())
            .collect();
        let doc_file = document_of(&dir);
        let mut created = 0;
        for patch in patches_of(&dir) {
            let slug = patch_slug(&file_name(&patch)).to_string();
            for finding in read_findings(&dir, &slug) {
                if finding.id.is_empty() || !seen.insert(finding.id.clone()) {
                    continue;
                }
                let finding = Finding {
                    target: Some(format!("patch:{slug}")),
                    ..finding
                };
                self.add_finding(&review.id, &dir, doc_file.as_deref(), &finding, None)?;
                created += 1;
            }
        }
        Ok(created)
    }

    fn add_finding(
        &self,
        review_id: &str,
        dir: &Path,
        doc_file: Option<&Path>,
        finding: &Finding,
        author: Option<Participant>,
    ) -> Result<()> {
        if self.store.get(review_id).is_none() {
            return Err(anyhow!("No review {review_id}."));
        }
        let anchor = match &finding.anchor {
            serde_json::Value::Null => serde_json::Value::Object(serde_json::Map::new()),
            anchor => anchor.clone(),
        };
        let location = location_from_anchor(dir, doc_file, finding.target.as_deref(), &anchor);
        let body = match &finding.verdict {
            Some(verdict) => format!("{}\n\n_{verdict}_", finding.body),
            None => finding.body.clone(),
        };
        let author = author.unwrap_or_else(|| Participant {
            id: "agent:review-bot".into(),
            kind: ParticipantKind::Agent,
            name: "review-bot".into(),
        });
        let options = ThreadOptions {
            status: Some(ThreadStatus::Proposed),
            origin: Some("finding".into()),
            source_id: Some(finding.id.clone()),
            severity: finding.severity.as_deref().and_then(parse_severity),
            labels: finding.labels.clone(),
            proposal: finding.proposal.as_ref().map(|proposal| Proposal {
                before: proposal.before.clone(),
                after: proposal.after.clone(),
                path: proposal.path.clone(),
                start_line: proposal.start_line,
                status: ProposalStatus::Pending,
            }),
            intent: None,
        };
        self.store
            .create_thread(review_id, location, body, author, options)?;
        Ok(())
    }

    fn export(&self, review_id: &str) -> Result<()> {
        let Some(review) = self.store.get(review_id) else {
            return Ok(());
        };
        let Some(bundle) = review.bundle.as_ref() else {
            return Ok(());
        };
        let dir = PathBuf::from(&bundle.dir);
        if !dir.exists() {
            return Ok(());
        }
        let document = match document_of(&dir) {
            Some(doc_file) => Some(StateDocument {
                slug: file_name(&doc_file),
                title: title_of(&doc_file)?,
                markdown: fs::read_to_string(&doc_file)
                    .with_context(|| format!("reading {}", doc_file.display()))?,
            }),
            None => None,
        };
        let file = bundle
            .store_path
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| dir.join("review.json"));
        let state = serde_json::to_string_pretty(&to_state_file(&review, document.as_ref()))?;
        let _exporting = self
            .exporting
            .lock()
            .map_err(|_| anyhow!("export lock poisoned"))?;
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = PathBuf::from(format!("{}.{}.tmp", file.display(), std::process::id()));
        fs::write(&temporary, state).with_context(|| format!("writing {}", temporary.display()))?;
        fs::rename(&temporary, &file).with_context(|| format!("writing {}", file.display()))?;
        Ok(())
    }

    /// New or edited findings files are ingested while the review is open.
    fn watch(shared: &Arc<Shared>, review_id: &str, dir: &Path) {
        let Ok(mut watchers) = shared.watchers.lock() else {
            return;
        };
        if watchers.contains_key(dir) {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let weak = Arc::downgrade(shared);
        let review_id = review_id.to_string();
        let watched = dir.to_path_buf();
        let thread_stop = stop.clone();
        // Taken before the thread starts, so a change made right after opening is never mistaken for the start.
        let known = findings_files(dir);
        let thread = thread::Builder::new()
            .name("co-review-findings-watch".into())
            .spawn(move || watch_findings(weak, &review_id, &watched, known, &thread_stop));
        match thread {
            Ok(thread) => {
                watchers.insert(
                    dir.to_path_buf(),
                    Watcher {
                        stop,
                        thread: Some(thread),
                    },
                );
            }
            Err(error) => log::error!("[co-review] cannot watch {}: {error}", dir.display()),
        }
    }

    fn unwatch(&self, dir: &Path) {
        let watcher = match self.watchers.lock() {
            Ok(mut watchers) => watchers.remove(dir),
            Err(_) => None,
        };
        if let Some(watcher) = watcher {
            watcher.stop();
        }
    }
}

/// Polls `dir`'s findings files and syncs once they have been quiet for `SYNC_DEBOUNCE`.
fn watch_findings(
    shared: Weak<Shared>,
    review_id: &str,
    dir: &Path,
    mut known: HashMap<PathBuf, (SystemTime, u64)>,
    stop: &AtomicBool,
) {
    let mut pending_since: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(WATCH_INTERVAL);
        let current = findings_files(dir);
        if current != known {
            known = current;
            pending_since = Some(Instant::now());
            continue;
        }
        if pending_since.is_some_and(|since| since.elapsed() >= SYNC_DEBOUNCE) {
            pending_since = None;
            let Some(shared) = shared.upgrade() else {
                return;
            };
            if let Err(error) = shared.sync_findings(review_id) {
                log::error!(
                    "[co-review] syncing findings of {} failed: {error:#}",
                    dir.display()
                );
            }
        }
    }
}

/// The findings files of a directory with their modification time and size.
fn findings_files(dir: &Path) -> HashMap<PathBuf, (SystemTime, u64)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter(|entry| is_findings_file(&entry.file_name().to_string_lossy()))
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            Some((entry.path(), (metadata.modified().ok()?, metadata.len())))
        })
        .collect()
}

fn is_findings_file(name: &str) -> bool {
    name.ends_with(".comments.json") || name.ends_with(".comments.jsonl")
}

/// Writes `review.json` for each review directory's review once its changes are quiet; stops
/// watching a directory whose review was deleted.
fn spawn_exporter(shared: &Arc<Shared>) {
    let changes = shared.store.subscribe_changes();
    let weak = Arc::downgrade(shared);
    let spawned = thread::Builder::new()
        .name("co-review-review-json".into())
        .spawn(move || {
            while let Ok(change) = changes.recv_blocking() {
                let mut pending: HashSet<String> = HashSet::new();
                let mut next = Some(change);
                loop {
                    while let Some(change) = next.take() {
                        let Some(shared) = weak.upgrade() else {
                            return;
                        };
                        match change {
                            ReviewChange::Changed { review } => {
                                if review.bundle.is_some() {
                                    pending.insert(review.id);
                                }
                            }
                            ReviewChange::Deleted {
                                review_id,
                                workspace_root,
                            } => {
                                pending.remove(&review_id);
                                if let Some(dir) = file_uri_to_path(&workspace_root) {
                                    shared.unwatch(&dir);
                                }
                            }
                        }
                        next = changes.try_recv().ok();
                    }
                    thread::sleep(EXPORT_DEBOUNCE);
                    next = changes.try_recv().ok();
                    if next.is_none() {
                        break;
                    }
                }
                let Some(shared) = weak.upgrade() else {
                    return;
                };
                for review_id in pending {
                    if let Err(error) = shared.export(&review_id) {
                        log::error!("[co-review] review.json export failed: {error:#}");
                    }
                }
            }
        });
    if let Err(error) = spawned {
        log::error!("[co-review] cannot start the review.json exporter: {error}");
    }
}

/// The branch a patch was made on, from `<slug>.md` or `PR.md` next to it.
fn branch_of_patch(patch: &Path) -> Option<String> {
    let directory = patch.parent()?;
    let slug = patch_slug(&file_name(patch)).to_string();
    let meta_file = [
        directory.join(format!("{slug}.md")),
        directory.join("PR.md"),
    ]
    .into_iter()
    .find(|file| file.exists())?;
    parse_patch_meta(&fs::read_to_string(meta_file).ok()?).branch
}

fn parse_severity(severity: &str) -> Option<Severity> {
    serde_json::from_value(serde_json::Value::String(severity.to_string())).ok()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The document of a review directory: the first of `DOCUMENT_NAMES`, else the first
/// `*.pseudocode.md`.
pub fn document_of(dir: &Path) -> Option<PathBuf> {
    if let Some(name) = DOCUMENT_NAMES.iter().find(|name| dir.join(name).exists()) {
        return Some(dir.join(name));
    }
    let mut pseudocode: Vec<String> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".pseudocode.md"))
        .collect();
    pseudocode.sort();
    pseudocode.first().map(|name| dir.join(name))
}

/// The `*.patch` / `*.diff` pages of a review directory, by name.
pub fn patches_of(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".patch") || name.ends_with(".diff"))
        .collect();
    names.sort();
    names.into_iter().map(|name| dir.join(name)).collect()
}

/// A document's title: its first `# heading` (without a `Feature:` prefix), else its folder's name.
pub fn title_of(file: &Path) -> Result<String> {
    static HEADING: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"(?m)^#\s+(.+?)\s*$").ok());
    static FEATURE: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"(?i)^Feature:\s*").ok());
    let text = fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let heading = HEADING
        .as_ref()
        .and_then(|heading| heading.captures(&text))
        .and_then(|captures| captures.get(1))
        .map(|heading| heading.as_str());
    Ok(match (heading, FEATURE.as_ref()) {
        (Some(heading), Some(feature)) => feature.replace(heading, "").into_owned(),
        (Some(heading), None) => heading.to_string(),
        (None, _) => file.parent().map(file_name).unwrap_or_default(),
    })
}

/// The findings of a patch: `<slug>.comments.json` (an array) or `.comments.jsonl`.
fn read_findings(dir: &Path, slug: &str) -> Vec<Finding> {
    for extension in [".comments.json", ".comments.jsonl"] {
        let file = dir.join(format!("{slug}{extension}"));
        if !file.exists() {
            continue;
        }
        let parsed = fs::read_to_string(&file)
            .map_err(anyhow::Error::from)
            .and_then(|text| {
                if extension.ends_with('l') {
                    findings::read_findings_jsonl(&text).map_err(Into::into)
                } else {
                    findings::read_findings_json(&text).map_err(Into::into)
                }
            });
        return match parsed {
            Ok(findings) => findings,
            Err(error) => {
                log::error!(
                    "[co-review] cannot read findings {}: {error:#}",
                    file.display()
                );
                Vec::new()
            }
        };
    }
    Vec::new()
}
