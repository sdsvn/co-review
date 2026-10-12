//! The reviews of one workspace (window tab), kept in step with the store.

use std::{
    cell::Cell,
    collections::BTreeMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use floem::{
    action::exec_after,
    ext_event::create_ext_action,
    reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith},
};
use review_model::{
    AgentTransport, CodeLocation, Review, ReviewCoverage, ThreadIntent,
};
use review_store::ReviewChange;

use super::ReviewCore;
use crate::workspace::{LapceWorkspace, LapceWorkspaceType};

/// The reviews a window tab shows: the workspace's own and the change reviews whose code folder it is (as
/// `ReviewStore::list_for_window`). Loaded off the UI thread, then updated one review at a time from store changes.
#[derive(Clone)]
pub struct ReviewWorkspaceData {
    /// The workspace's folder resolved through symbolic links, as the store keys reviews; `None` without a local
    /// folder.
    pub root: Option<PathBuf>,
    /// Every review of the workspace, archived ones too, most recently updated first.
    pub reviews: RwSignal<im::Vector<Arc<Review>>>,
    /// Whether `reviews` has been read from the store yet.
    pub loaded: RwSignal<bool>,
    /// The review the panel and the editor show (the review switcher's choice).
    pub active: RwSignal<Option<String>>,
    /// Comments the reviewer is writing and has not sent yet.
    pub drafts: RwSignal<im::Vector<ReviewDraft>>,
    /// Which files of the repository the active review has seen (repository reviews only), refreshed shortly after
    /// the review's viewed files change.
    pub coverage: RwSignal<Option<ReviewCoverage>>,
    /// The window is closing for an agent that is done with it, unless the reviewer keeps it open.
    pub closing: RwSignal<Option<ClosingNotice>>,
    /// The thread last opened from the panel or Go to Comment…, highlighted in the panel.
    pub selected_thread: RwSignal<Option<String>>,
    /// The submit form is open in the panel.
    pub submitting: RwSignal<bool>,
    /// The change shown in the file explorer, when the window is on a change review's code folder.
    pub change: super::changed_files::ChangedFiles,
}

/// A comment being written: where it goes, its text so far and whether it asks the agent.
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewDraft {
    pub id: String,
    pub review_id: String,
    pub location: CodeLocation,
    pub body: String,
    pub intent: ThreadIntent,
}

/// Why the window closes, and in how many seconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosingNotice {
    pub message: String,
    pub seconds: u32,
}

impl ReviewWorkspaceData {
    pub fn new(cx: Scope, workspace: &LapceWorkspace) -> Self {
        let root = match (&workspace.kind, &workspace.path) {
            (LapceWorkspaceType::Local, Some(path)) => {
                Some(review_store::real_path(path))
            }
            _ => None,
        };
        let reviews = cx.create_rw_signal(im::Vector::new());
        let active = cx.create_rw_signal(None);
        let data = Self {
            change: super::changed_files::ChangedFiles::new(cx, root.clone(), reviews, active),
            root,
            reviews,
            loaded: cx.create_rw_signal(false),
            active,
            drafts: cx.create_rw_signal(im::Vector::new()),
            coverage: cx.create_rw_signal(None),
            closing: cx.create_rw_signal(None),
            selected_thread: cx.create_rw_signal(None),
            submitting: cx.create_rw_signal(false),
        };
        if let (Some(core), Some(root)) = (ReviewCore::get(), data.root.clone()) {
            data.load(cx, &core, root.clone());
            data.keep_active(cx);
            data.follow_coverage(cx, root);
        }
        data
    }

    /// The active review (tracked).
    pub fn active_review(&self) -> Option<Arc<Review>> {
        let active = self.active.get()?;
        self.reviews
            .with(|reviews| reviews.iter().find(|review| review.id == active).cloned())
    }

    /// The active review, untracked.
    pub fn active_review_untracked(&self) -> Option<Arc<Review>> {
        let active = self.active.get_untracked()?;
        self.reviews.with_untracked(|reviews| {
            reviews.iter().find(|review| review.id == active).cloned()
        })
    }

    /// The archived reviews, most recently archived first.
    pub fn archived_reviews(&self) -> Vec<Arc<Review>> {
        let mut archived: Vec<Arc<Review>> = self.reviews.with_untracked(|reviews| {
            reviews
                .iter()
                .filter(|review| review.archived_at.is_some())
                .cloned()
                .collect()
        });
        archived.sort_by_key(|review| std::cmp::Reverse(review.archived_at));
        archived
    }

    /// A path of the workspace as reviews record it: relative to the root, with `/`; `None` outside it.
    pub fn relative_path(&self, path: &Path) -> Option<String> {
        let root = self.root.as_deref()?;
        let path = review_store::real_path(path);
        let relative = path.strip_prefix(root).ok()?;
        let parts: Vec<String> = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        Some(parts.join("/"))
    }

    /// Keeps a review active: the first open one when the active one is gone (deleted, or never chosen).
    fn keep_active(&self, cx: Scope) {
        let (reviews, loaded, active) = (self.reviews, self.loaded, self.active);
        cx.create_effect(move |_| {
            if !loaded.get() {
                return;
            }
            let current = active.get();
            let replacement = reviews.with(|reviews| {
                let present = current
                    .as_ref()
                    .is_some_and(|id| reviews.iter().any(|review| &review.id == id));
                if present {
                    return None;
                }
                Some(
                    reviews
                        .iter()
                        .find(|review| review.archived_at.is_none())
                        .map(|review| review.id.clone()),
                )
            });
            if let Some(replacement) = replacement
                && replacement != current
            {
                active.set(replacement);
            }
        });
    }

    /// Recomputes the coverage of the active repository review, off the UI thread, 400 ms after its viewed files
    /// change (a burst of changes computes once).
    fn follow_coverage(&self, cx: Scope, root: PathBuf) {
        let data = self.clone();
        let coverage = self.coverage;
        let viewed = cx.create_memo(move |_| {
            let review = data.active_review()?;
            if review.bundle.is_some() {
                return None;
            }
            let viewed: BTreeMap<String, u64> = review
                .viewed
                .as_ref()
                .map(|viewed| viewed.iter().map(|(path, at)| (path.clone(), *at)).collect())
                .unwrap_or_default();
            Some((review.id.clone(), viewed))
        });
        let generation = Rc::new(Cell::new(0u64));
        cx.create_effect(move |_| {
            let Some((_, viewed)) = viewed.get() else {
                coverage.set(None);
                return;
            };
            let this = generation.get() + 1;
            generation.set(this);
            let (generation, root) = (generation.clone(), root.clone());
            exec_after(Duration::from_millis(400), move |_| {
                if generation.get() != this {
                    return;
                }
                let send = create_ext_action(cx, move |computed: Option<ReviewCoverage>| {
                    if generation.get() == this {
                        tracing::debug!(
                            "coverage: {:?}",
                            computed.as_ref().map(|coverage| (coverage.viewed, coverage.total))
                        );
                        coverage.set(computed);
                    }
                });
                let spawned = std::thread::Builder::new()
                    .name("ReviewCoverage".to_owned())
                    .spawn(move || send(compute_coverage(&root, viewed)));
                if let Err(error) = spawned {
                    tracing::error!("could not compute the review's coverage: {error}");
                }
            });
        });
    }

    /// The reviews not archived, most recently updated first.
    pub fn open_reviews(&self) -> Vec<Arc<Review>> {
        self.reviews.with_untracked(|reviews| {
            reviews
                .iter()
                .filter(|review| review.archived_at.is_none())
                .cloned()
                .collect()
        })
    }

    fn load(&self, cx: Scope, core: &ReviewCore, root: PathBuf) {
        let uri = review_store::path_to_file_uri(&root.to_string_lossy());
        let (reviews, loaded) = (self.reviews, self.loaded);
        let shown_root = root.clone();
        let send = create_ext_action(cx, move |listed: Vec<Review>| {
            tracing::debug!("{} reviews in {}", listed.len(), shown_root.display());
            reviews.set(listed.into_iter().map(Arc::new).collect());
            loaded.set(true);
        });
        let store = Arc::clone(&core.services.store);
        let listing_uri = uri.clone();
        let listed = std::thread::Builder::new()
            .name("ReviewList".to_owned())
            .spawn(move || send(store.list_for_window(&listing_uri)));
        if let Err(error) = listed {
            tracing::error!(
                "could not list the reviews of {}: {error}",
                root.display()
            );
        }

        let changes = core.changes;
        let active = self.active;
        cx.create_effect(move |_| {
            changes.with(|change| {
                if !loaded.get_untracked() {
                    return;
                }
                match change {
                    Some(ReviewChange::Changed { review }) => {
                        let shown = belongs(review, &uri);
                        let previous = reviews.with_untracked(|reviews| {
                            reviews
                                .iter()
                                .find(|existing| existing.id == review.id)
                                .cloned()
                        });
                        if !shown && previous.is_none() {
                            return;
                        }
                        // An agent from a harness just joined this review: show it.
                        let joined = review.agent.as_ref().filter(|agent| {
                            agent.transport == Some(AgentTransport::Mcp)
                        });
                        if shown
                            && let Some(agent) = joined
                            && previous
                                .as_ref()
                                .and_then(|previous| previous.agent.as_ref())
                                .is_none_or(|previous| previous.id != agent.id)
                        {
                            active.set(Some(review.id.clone()));
                        }
                        reviews.update(|reviews| {
                            reviews.retain(|existing| existing.id != review.id);
                            if shown {
                                let index = reviews
                                    .iter()
                                    .position(|existing| {
                                        existing.updated_at <= review.updated_at
                                    })
                                    .unwrap_or(reviews.len());
                                reviews.insert(index, Arc::new(review.clone()));
                            }
                        });
                    }
                    Some(ReviewChange::Deleted { review_id, .. })
                        if reviews.with_untracked(|reviews| {
                            reviews.iter().any(|review| &review.id == review_id)
                        }) =>
                    {
                        reviews.update(|reviews| {
                            reviews.retain(|review| &review.id != review_id)
                        });
                    }
                    _ => {}
                }
            });
        });
    }
}

/// Whether a window of the workspace `uri` shows `review` (the rule of `ReviewStore::list_for_window`).
fn belongs(review: &Review, uri: &str) -> bool {
    review.workspace_root == uri
        || review
            .bundle
            .as_ref()
            .and_then(|bundle| bundle.code.as_deref())
            .is_some_and(|code| review_store::path_to_file_uri(code) == uri)
}

/// The coverage of the repository at `root` given the review's viewed files (path → when, in ms).
fn compute_coverage(root: &Path, viewed: BTreeMap<String, u64>) -> Option<ReviewCoverage> {
    let index = review_index::RepoIndex::new();
    match index.build(root) {
        Ok(map) => {
            let viewed = viewed
                .into_iter()
                .map(|(path, at)| (path, at as f64))
                .collect();
            Some(index.coverage(&map, &viewed))
        }
        Err(error) => {
            tracing::error!("no coverage for {}: {error:#}", root.display());
            None
        }
    }
}
