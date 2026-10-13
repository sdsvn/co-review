//! Drafts: comments the reviewer is writing and has not sent. They live in the window tab's
//! `ReviewWorkspaceData::drafts`, are kept per review in `<workspace dir>/drafts.json` (beside the workspace's reviews
//! in `$CO_REVIEW_HOME`) and come back after a restart. Creating, editing, sending and discarding them is here too.

use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

use floem::{
    action::exec_after,
    ext_event::create_ext_action,
    reactive::{SignalGet, SignalUpdate, SignalWith},
};
use review_model::{CodeLocation, Review, ThreadIntent};
use review_store::CreateReviewParams;

use super::{ReviewCore, thread::run, workspace::ReviewDraft};
use crate::{id::WindowTabId, window_tab::WindowTabData};

thread_local! {
    static KEPT: RefCell<HashSet<WindowTabId>> = RefCell::new(HashSet::new());
}

/// Loads the window tab's drafts and saves them whenever they change (300 ms after the last change). Called for every
/// window tab; only the first call per tab does anything.
pub fn keep(window_tab: &WindowTabData) {
    let first = KEPT.with(|kept| kept.borrow_mut().insert(window_tab.window_tab_id));
    if !first {
        return;
    }
    let (Some(core), Some(root)) = (ReviewCore::get(), window_tab.review.root.clone()) else {
        return;
    };
    let file = core
        .services
        .store
        .workspace_dir(&review_store::path_to_file_uri(&root.to_string_lossy()))
        .join("drafts.json");
    let drafts = window_tab.review.drafts;
    let cx = window_tab.scope;
    let loaded = Rc::new(Cell::new(false));
    {
        let loaded = loaded.clone();
        let send = create_ext_action(cx, move |restored: Vec<ReviewDraft>| {
            drafts.update(|drafts| {
                for draft in restored {
                    if !drafts.iter().any(|existing| existing.id == draft.id) {
                        drafts.push_back(draft);
                    }
                }
            });
            loaded.set(true);
        });
        let file = file.clone();
        let read = std::thread::Builder::new()
            .name("ReviewDrafts".to_owned())
            .spawn(move || {
                let records = match std::fs::read_to_string(&file) {
                    Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                        tracing::error!("{} is not readable: {error}", file.display());
                        Vec::new()
                    }),
                    Err(_) => Vec::new(),
                };
                send(records);
            });
        if let Err(error) = read {
            tracing::error!("drafts not loaded: {error}");
        }
    }
    let generation = Rc::new(Cell::new(0u64));
    cx.create_effect(move |_| {
        let records = drafts.get();
        if !loaded.get() {
            return;
        }
        let this = generation.get() + 1;
        generation.set(this);
        let (generation, file) = (generation.clone(), file.clone());
        exec_after(Duration::from_millis(300), move |_| {
            if generation.get() == this {
                save(file, records);
            }
        });
    });
}

fn save(file: PathBuf, records: im::Vector<ReviewDraft>) {
    let written = std::thread::Builder::new()
        .name("ReviewDraftsSave".to_owned())
        .spawn(move || {
            let text = match serde_json::to_string_pretty(&records) {
                Ok(text) => text,
                Err(error) => {
                    tracing::error!("drafts not saved: {error}");
                    return;
                }
            };
            let result = file
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| {
                    let temporary = file.with_extension("json.tmp");
                    std::fs::write(&temporary, text)?;
                    std::fs::rename(&temporary, &file)
                });
            if let Err(error) = result {
                tracing::error!("drafts not saved to {}: {error}", file.display());
            }
        });
    if let Err(error) = written {
        tracing::error!("drafts not saved: {error}");
    }
}

/// Opens a draft at `location` in the active review (or focuses the draft already there), creating a review for
/// the workspace first when it has none. Returns the draft's id through `opened`.
pub fn add(
    window_tab: &WindowTabData,
    location: CodeLocation,
    intent: ThreadIntent,
    opened: impl FnOnce(String) + 'static,
) {
    keep(window_tab);
    let drafts = window_tab.review.drafts;
    let place = move |review_id: String| opened(insert(drafts, &review_id, location, intent));
    if let Some(review) = window_tab.review.active_review_untracked() {
        place(review.id.clone());
        return;
    }
    // No review yet: start one for the repository, then place the draft in it.
    let Some(root) = window_tab.review.root.clone() else {
        tracing::warn!("comments need a folder opened as the workspace");
        return;
    };
    let active = window_tab.review.active;
    let send = create_ext_action(window_tab.scope, move |review: Review| {
        active.set(Some(review.id.clone()));
        place(review.id);
    });
    run(window_tab, "starting a review", move |backend| {
        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "the repository".to_string());
        send(backend.services.store.create(CreateReviewParams {
            workspace_root: review_store::path_to_file_uri(&root.to_string_lossy()),
            title: format!("Review of {name}"),
            scope: review_model::ReviewScope::Repository,
            author: backend.user(),
        })?);
        Ok(())
    });
}

/// Opens a draft at `location` in review `review_id` (or returns the draft already there, now with `intent`). Its id.
pub fn add_to(
    window_tab: &WindowTabData,
    review_id: &str,
    location: CodeLocation,
    intent: ThreadIntent,
) -> String {
    keep(window_tab);
    insert(window_tab.review.drafts, review_id, location, intent)
}

fn insert(
    drafts: floem::reactive::RwSignal<im::Vector<ReviewDraft>>,
    review_id: &str,
    location: CodeLocation,
    intent: ThreadIntent,
) -> String {
    let existing = drafts.with_untracked(|drafts| {
        drafts
            .iter()
            .find(|draft| draft.review_id == review_id && draft.location == location)
            .map(|draft| draft.id.clone())
    });
    match existing {
        Some(id) => {
            drafts.update(|drafts| {
                if let Some(draft) = drafts.iter_mut().find(|draft| draft.id == id) {
                    draft.intent = intent;
                }
            });
            id
        }
        None => {
            let id = new_id();
            drafts.update(|drafts| {
                drafts.push_back(ReviewDraft {
                    id: id.clone(),
                    review_id: review_id.to_string(),
                    location,
                    body: String::new(),
                    intent,
                })
            });
            id
        }
    }
}

pub fn set_body(window_tab: &WindowTabData, draft_id: &str, body: String) {
    window_tab.review.drafts.update(|drafts| {
        if let Some(draft) = drafts.iter_mut().find(|draft| draft.id == draft_id) {
            draft.body = body;
        }
    });
}

pub fn discard(window_tab: &WindowTabData, draft_id: &str) {
    window_tab
        .review
        .drafts
        .update(|drafts| drafts.retain(|draft| draft.id != draft_id));
}

/// Sends the draft as a new thread (a question when `intent` asks and the review has an agent), and asks the
/// review's ACP agent when it should answer. The draft comes back if the thread could not be created.
pub fn submit(window_tab: &WindowTabData, draft_id: &str, intent: ThreadIntent) {
    let drafts = window_tab.review.drafts;
    let Some(draft) = drafts.with_untracked(|drafts| {
        drafts.iter().find(|draft| draft.id == draft_id).cloned()
    }) else {
        return;
    };
    if draft.body.trim().is_empty() {
        return;
    }
    let has_agent = super::thread::review_of(window_tab, &draft.review_id)
        .is_some_and(|review| review.agent.is_some());
    let intent = if has_agent { intent } else { ThreadIntent::Comment };
    discard(window_tab, draft_id);
    let restore = {
        let draft = draft.clone();
        create_ext_action(window_tab.scope, move |()| {
            drafts.update(|drafts| drafts.push_back(draft));
        })
    };
    run(window_tab, "sending the comment", move |backend| {
        backend
            .start_thread(&draft.review_id, draft.location.clone(), draft.body.trim().to_string(), intent)
            .map(drop)
            .inspect_err(|_| restore(()))
    });
}

fn new_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    thread_local! {
        static COUNTER: Cell<u32> = const { Cell::new(0) };
    }
    let count = COUNTER.with(|counter| {
        counter.set(counter.get() + 1);
        counter.get()
    });
    format!("draft-{millis}-{count}")
}

