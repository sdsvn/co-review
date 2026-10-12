//! Overview pages on disk: `<workspace dir>/pages/<review>/overview.md`, written with what is cheap
//! at once, the slow section (repository overview or blast radius) filled in by a background
//! build, and kept up to date as the review changes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use parking_lot::Mutex;
use review_bundle::discovery::patches_of;
use review_bundle::patch::{PatchFile, changed_lines, parse_patch, parse_patch_meta};
use review_graph::{BlastRadius, GraphState, GraphifyGraph, build_graph};
use review_index::{FileEntry, RepoIndex};
use review_store::{ReviewChange, ReviewStore};

use crate::overview_page::{OverviewParts, PatchFileSummary, PatchSummary, Radius, overview_page};
use crate::repo_overview::{OverviewInput, repository_overview};
use crate::text::file_uri_to_path;

/// Changes come in bursts (an agent streaming a reply): a page is written once per burst.
pub const COALESCE_DELAY: Duration = Duration::from_millis(500);

/// Builds the slow sections of overview pages. They take seconds on a small repository and half a
/// minute on a large one, so they run on a background thread and never block a page.
pub trait SectionBuilder: Send + Sync + 'static {
    /// A repository review's repository overview. `viewed`: file → when the reviewer marked it viewed.
    fn repository_overview(&self, root: &Path, viewed: &HashMap<String, u64>) -> Result<String>;

    /// A change's blast radius in the repository; `None` when there is no graph.
    fn blast_radius(
        &self,
        repository: &Path,
        changed_lines: &BTreeMap<String, Vec<u32>>,
    ) -> Result<Option<BlastRadius>>;
}

/// The sections from the repository itself: its Tree-sitter map and its Graphify graph.
pub struct RepositorySections;

const MAX_CHILDREN: usize = 12;

/// A file's outline line, as `review_index` renders it for agents.
fn outline(entry: &FileEntry) -> String {
    entry
        .symbols
        .iter()
        .map(|(name, members)| {
            if members.is_empty() {
                return name.clone();
            }
            let shown = &members[..members.len().min(MAX_CHILDREN)];
            let more = if shown.len() < members.len() {
                ", …"
            } else {
                ""
            };
            format!("{name} {{ {}{more} }}", shown.join(", "))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

impl SectionBuilder for RepositorySections {
    fn repository_overview(&self, root: &Path, viewed: &HashMap<String, u64>) -> Result<String> {
        let index = RepoIndex::new();
        let (map, graph) = thread::scope(|scope| {
            // The graph is built or refreshed while the map is.
            let graph = scope.spawn(|| build_graph(root));
            let map = index.build(root);
            (map, graph.join().unwrap_or(GraphState::Failed))
        });
        let map = map.context("building the repository map")?;
        let viewed = viewed
            .iter()
            .map(|(file, at)| (file.clone(), *at as f64))
            .collect();
        let coverage = index.coverage(&map, &viewed);
        let mut outlines: Vec<(String, String)> = map
            .files
            .iter()
            .map(|(file, entry)| (file.clone(), outline(entry)))
            .collect();
        outlines.sort();
        repository_overview(&OverviewInput {
            root,
            coverage: &coverage,
            outlines: &outlines,
            graph: Some(graph),
        })
    }

    fn blast_radius(
        &self,
        repository: &Path,
        changed_lines: &BTreeMap<String, Vec<u32>>,
    ) -> Result<Option<BlastRadius>> {
        if build_graph(repository) != GraphState::Built {
            return Ok(None);
        }
        let graph = std::fs::read_to_string(repository.join("graphify-out").join("graph.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<GraphifyGraph>(&text).ok());
        Ok(graph.and_then(|graph| review_graph::blast_radius(&graph, changed_lines)))
    }
}

enum Section {
    Repository {
        root: PathBuf,
        viewed: HashMap<String, u64>,
    },
    Radius {
        repository: PathBuf,
        changed_lines: BTreeMap<String, Vec<u32>>,
    },
}

#[derive(Default)]
struct SectionState {
    /// Repository sections by review: built when the page is opened, reused when the review changes.
    repositories: HashMap<String, String>,
    /// Blast radii by review, the same way.
    radii: HashMap<String, Radius>,
    /// The reviews whose slow section is being built.
    building: HashSet<String>,
}

/// Writes review overview pages and keeps every written one up to date as its review changes.
pub struct OverviewPages {
    store: Arc<ReviewStore>,
    sections: Arc<dyn SectionBuilder>,
    state: Mutex<SectionState>,
    /// One page write at a time, so a page with the finished section is never overwritten by an
    /// older one that still says it is being built.
    writing: Mutex<()>,
}

impl OverviewPages {
    /// Starts following the store's changes on a thread of its own: written pages are rewritten
    /// when their review changes (once per burst) and deleted with it.
    pub fn new(store: Arc<ReviewStore>, sections: Arc<dyn SectionBuilder>) -> Arc<Self> {
        let changes = store.subscribe_changes();
        let pages = Arc::new(Self {
            store,
            sections,
            state: Mutex::new(SectionState::default()),
            writing: Mutex::new(()),
        });
        let weak = Arc::downgrade(&pages);
        let spawned = thread::Builder::new()
            .name("overview-pages".to_string())
            .spawn(move || follow_changes(weak, changes));
        if let Err(error) = spawned {
            log::error!("[co-review] overview pages can't follow review changes: {error}");
        }
        pages
    }

    /// Where a review's overview page is written.
    pub fn file_of(&self, workspace_root: &str, review_id: &str) -> PathBuf {
        self.store
            .workspace_dir(workspace_root)
            .join("pages")
            .join(review_id)
            .join("overview.md")
    }

    /// Writes the review's overview page and returns its file; `None` for a review that doesn't
    /// exist, or a review directory without patches (a design or a knowledge bundle: its document
    /// is its front page). `fresh`: rebuild the slow section (the repository overview, or a
    /// change's blast radius) in the background; until it is ready the page shows the one built
    /// before, or says it is being built. Reads files: call it off the foreground thread.
    pub fn write(self: &Arc<Self>, review_id: &str, fresh: bool) -> Result<Option<PathBuf>> {
        let _writing = self.writing.lock();
        let Some(review) = self.store.get(review_id) else {
            return Ok(None);
        };
        let directory = review.bundle.as_ref().map(|bundle| bundle.dir.clone());
        let patch_files = match &directory {
            Some(directory) => patches_in(Path::new(directory))?,
            None => Vec::new(),
        };
        if directory.is_some() && patch_files.is_empty() {
            return Ok(None);
        }
        let mut parts = OverviewParts::default();
        let root = match &directory {
            Some(directory) => {
                parts.pull_request = std::fs::read_to_string(Path::new(directory).join("PR.md"))
                    .ok()
                    .map(|text| parse_patch_meta(&text))
                    .filter(|meta| {
                        meta.title.as_deref().is_some_and(|title| !title.is_empty())
                            || meta.url.as_deref().is_some_and(|url| !url.is_empty())
                            || meta.number.is_some_and(|number| number != 0)
                    });
                let mut all: Vec<PatchFile> = Vec::new();
                for file in &patch_files {
                    let files = parse_patch(&std::fs::read_to_string(file).unwrap_or_default());
                    let name = file
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let name = name.as_str();
                    let name = name
                        .strip_suffix(".patch")
                        .or_else(|| name.strip_suffix(".diff"))
                        .unwrap_or(name);
                    parts.patches.push(PatchSummary {
                        name: name.to_string(),
                        files: files
                            .iter()
                            .map(|file| PatchFileSummary {
                                path: file.path.clone(),
                                added: file.added,
                                removed: file.removed,
                            })
                            .collect(),
                    });
                    all.extend(files);
                }
                let repository = review
                    .bundle
                    .as_ref()
                    .and_then(|bundle| bundle.repo.clone())
                    .filter(|repository| !repository.is_empty());
                if let Some(repository) = repository {
                    // The graph is built or refreshed when the page is opened; rewrites after review changes reuse it.
                    if fresh || !self.state.lock().radii.contains_key(review_id) {
                        self.build(
                            review_id,
                            Section::Radius {
                                repository: PathBuf::from(repository),
                                changed_lines: changed_lines(&all),
                            },
                        );
                    }
                    parts.radius = self.state.lock().radii.get(review_id).cloned();
                }
                directory.clone()
            }
            None => {
                let root = file_uri_to_path(&review.workspace_root);
                if fresh || !self.state.lock().repositories.contains_key(review_id) {
                    self.build(
                        review_id,
                        Section::Repository {
                            root: PathBuf::from(&root),
                            viewed: review.viewed.clone().unwrap_or_default(),
                        },
                    );
                }
                parts.repository = self
                    .state
                    .lock()
                    .repositories
                    .get(review_id)
                    .filter(|repository| !repository.is_empty())
                    .cloned();
                root
            }
        };
        parts.building = self.state.lock().building.contains(review_id);
        let file = self.file_of(&review.workspace_root, &review.id);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&file, overview_page(&review, &root, &parts))
            .with_context(|| format!("writing {}", file.display()))?;
        Ok(Some(file))
    }

    /// Builds a review's slow section on a background thread, one build per review at a time,
    /// then writes the page again so the section shows. A failed build settles the section (no
    /// repository overview, no graph) so the page stops saying it is being built.
    fn build(self: &Arc<Self>, review_id: &str, section: Section) {
        if !self.state.lock().building.insert(review_id.to_string()) {
            return;
        }
        let pages = Arc::clone(self);
        let id = review_id.to_string();
        let spawned = thread::Builder::new()
            .name("overview-section".to_string())
            .spawn(move || {
                pages.build_section(&id, section);
                pages.state.lock().building.remove(&id);
                // A deleted review is not written again: `write` returns before writing when the store has no review.
                if let Err(error) = pages.write(&id, false) {
                    log::error!("[co-review] overview page failed: {error:#}");
                }
            });
        if let Err(error) = spawned {
            log::error!("[co-review] overview section failed to start: {error}");
            self.state.lock().building.remove(review_id);
        }
    }

    fn build_section(&self, review_id: &str, section: Section) {
        match section {
            Section::Repository { root, viewed } => {
                let repository = self
                    .sections
                    .repository_overview(&root, &viewed)
                    .unwrap_or_else(|error| {
                        log::error!("[co-review] overview section failed: {error:#}");
                        String::new()
                    });
                self.state
                    .lock()
                    .repositories
                    .insert(review_id.to_string(), repository);
            }
            Section::Radius {
                repository,
                changed_lines,
            } => match self.sections.blast_radius(&repository, &changed_lines) {
                Ok(radius) => {
                    let files = radius.as_ref().map(|radius| radius.files.clone());
                    self.state.lock().radii.insert(
                        review_id.to_string(),
                        radius.map_or(Radius::NoGraph, Radius::Graph),
                    );
                    if let Some(files) = files {
                        self.follow_related_files(review_id, files);
                    }
                }
                Err(error) => {
                    log::error!("[co-review] overview section failed: {error:#}");
                    self.state
                        .lock()
                        .radii
                        .insert(review_id.to_string(), Radius::NoGraph);
                }
            },
        }
    }

    /// The review's Related files follow the graph, e.g. once Graphify built one the open didn't have.
    fn follow_related_files(&self, review_id: &str, files: Vec<String>) {
        let Some(mut bundle) = self.store.get(review_id).and_then(|review| review.bundle) else {
            return;
        };
        if bundle.related.as_deref().unwrap_or_default() == files.as_slice() {
            return;
        }
        bundle.related = Some(files);
        if let Err(error) = self.store.set_bundle(review_id, Some(bundle)) {
            log::error!("[co-review] updating related files failed: {error}");
        }
    }

    fn forget(&self, workspace_root: &str, review_id: &str) {
        {
            let mut state = self.state.lock();
            state.repositories.remove(review_id);
            state.radii.remove(review_id);
        }
        let file = self.file_of(workspace_root, review_id);
        if let Some(directory) = file.parent()
            && let Err(error) = std::fs::remove_dir_all(directory)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::error!(
                "[co-review] removing {} failed: {error}",
                directory.display()
            );
        }
    }

    /// A page that was written (in this run or an earlier one) follows its review.
    fn rewrite(self: &Arc<Self>, workspace_root: &str, review_id: &str) {
        if !self.file_of(workspace_root, review_id).exists() {
            return;
        }
        if let Err(error) = self.write(review_id, false) {
            log::error!("[co-review] overview page failed: {error:#}");
        }
    }
}

/// The patches of a review directory, by name.
fn patches_in(directory: &Path) -> Result<Vec<PathBuf>> {
    let names: Vec<String> = std::fs::read_dir(directory)
        .with_context(|| format!("reading {}", directory.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    Ok(patches_of(&names)
        .into_iter()
        .map(|name| directory.join(name))
        .collect())
}

/// Rewrites pages after a burst of changes to their review settles, and deletes them with it.
/// Ends when the store or the pages are gone.
fn follow_changes(pages: Weak<OverviewPages>, changes: smol::channel::Receiver<ReviewChange>) {
    // Review id → (when to write, its workspace root).
    let mut pending: HashMap<String, (Instant, String)> = HashMap::new();
    loop {
        let next = pending.values().map(|(at, _)| *at).min();
        let received = smol::block_on(async {
            let change = async { Some(changes.recv().await) };
            match next {
                Some(at) => {
                    smol::future::or(change, async {
                        smol::Timer::at(at).await;
                        None
                    })
                    .await
                }
                None => change.await,
            }
        });
        let Some(pages) = pages.upgrade() else {
            return;
        };
        match received {
            Some(Ok(ReviewChange::Changed { review })) => {
                pending.insert(
                    review.id.clone(),
                    (
                        Instant::now() + COALESCE_DELAY,
                        review.workspace_root.clone(),
                    ),
                );
            }
            Some(Ok(ReviewChange::Deleted {
                review_id,
                workspace_root,
            })) => {
                pending.remove(&review_id);
                pages.forget(&workspace_root, &review_id);
            }
            Some(Err(_)) => return,
            None => {}
        }
        let now = Instant::now();
        let due: Vec<String> = pending
            .iter()
            .filter(|(_, (at, _))| *at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            if let Some((_, workspace_root)) = pending.remove(&id) {
                pages.rewrite(&workspace_root, &id);
            }
        }
    }
}
