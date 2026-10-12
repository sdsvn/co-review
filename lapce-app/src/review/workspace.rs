//! The reviews of one workspace (window tab), kept in step with the store.

use std::{path::PathBuf, sync::Arc};

use floem::{
    ext_event::create_ext_action,
    reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith},
};
use review_model::Review;
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
}

impl ReviewWorkspaceData {
    pub fn new(cx: Scope, workspace: &LapceWorkspace) -> Self {
        let root = match (&workspace.kind, &workspace.path) {
            (LapceWorkspaceType::Local, Some(path)) => {
                Some(review_store::real_path(path))
            }
            _ => None,
        };
        let data = Self {
            root,
            reviews: cx.create_rw_signal(im::Vector::new()),
            loaded: cx.create_rw_signal(false),
        };
        if let (Some(core), Some(root)) = (ReviewCore::get(), data.root.clone()) {
            data.load(cx, &core, root);
        }
        data
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
        cx.create_effect(move |_| {
            changes.with(|change| {
                if !loaded.get_untracked() {
                    return;
                }
                match change {
                    Some(ReviewChange::Changed { review }) => {
                        let shown = belongs(review, &uri);
                        let present = reviews.with_untracked(|reviews| {
                            reviews.iter().any(|existing| existing.id == review.id)
                        });
                        if !shown && !present {
                            return;
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
