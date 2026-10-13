//! The app's windows for the MCP server ([`review_mcp::ReviewWindows`]): showing a review opens its workspace's
//! window, or focuses the one already open, with the review's overview page in front (a change review's pull-request
//! page in front of it; a design's or knowledge bundle's document instead of an overview).

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use floem::{
    WindowIdExt,
    reactive::{SignalGet, SignalUpdate, untrack},
};
use review_mcp::{ReviewWindows, SHOWN_NOTE};
use parking_lot::Mutex;
use review_store::ReviewStore;

use super::{ReviewCore, ReviewPage, reveal_page};
use crate::{app::AppData, window::WindowData, window_tab::WindowTabData};

/// What the MCP server asks of the app's windows, handled on the UI thread.
#[derive(Clone, Debug)]
pub enum WindowRequest {
    /// Show `review_id`'s `pages` (the last in front, see [`pages_to_show`]) in the window of `workspace` (resolved
    /// through symbolic links).
    Show {
        review_id: String,
        workspace: PathBuf,
        pages: Vec<ReviewPage>,
    },
    /// The agent session is done with the window of `workspace_root` (a file URI), or with all of its windows.
    Done {
        session_id: String,
        message: String,
        workspace_root: Option<String>,
    },
    /// Another agent session uses a review of `workspace` (resolved) while its window counts down: it stays open.
    Rejoined { workspace: PathBuf },
}

/// Called on the MCP server's threads: forwards each request to the UI thread.
pub(super) struct AppWindows {
    requests: mpsc::Sender<WindowRequest>,
    store: Arc<ReviewStore>,
    /// Started in the background (by an agent): windows agents showed close when those agents are done.
    background: bool,
    /// Workspace (file URI) → the agent sessions that showed its window, in a background start.
    shown_by: Mutex<HashMap<String, HashSet<String>>>,
    /// Workspace (file URI) → the session that was done with its window last, and when: the window counts down to
    /// closing, unless another session rejoins it ([`ReviewWindows::joined`]).
    ending: Mutex<HashMap<String, (String, Instant)>>,
}

impl AppWindows {
    pub(super) fn new(
        requests: mpsc::Sender<WindowRequest>,
        store: Arc<ReviewStore>,
        background: bool,
    ) -> Self {
        Self {
            requests,
            store,
            background,
            shown_by: Mutex::default(),
            ending: Mutex::default(),
        }
    }

    fn send(&self, request: WindowRequest) {
        if self.requests.send(request).is_err() {
            tracing::error!(
                "the app's windows are gone; a review request was dropped"
            );
        }
    }
}

impl ReviewWindows for AppWindows {
    fn show(&self, review_id: &str, workspace: &Path, session_id: &str) -> String {
        if self.background {
            let key = workspace_key(workspace);
            self.ending.lock().remove(&key);
            self.shown_by.lock().entry(key).or_default().insert(session_id.to_string());
        }
        // Read here, off the UI thread: the review, its patches and its document decide the pages.
        let pages = pages_to_show(review_id, self.store.get(review_id).as_ref());
        self.send(WindowRequest::Show {
            review_id: review_id.to_string(),
            workspace: review_store::real_path(workspace),
            pages,
        });
        SHOWN_NOTE.to_string()
    }

    /// A window closes only when every session that showed it is done, and only in a background start.
    fn done(&self, session_id: &str, message: &str, workspace_root: Option<&str>) {
        let wanted = workspace_root
            .and_then(review_store::file_uri_to_path)
            .map(|path| workspace_key(Path::new(&path)));
        let closing: Vec<String> = {
            let mut shown_by = self.shown_by.lock();
            let roots: Vec<String> = shown_by
                .iter_mut()
                .filter(|(root, _)| wanted.as_ref().is_none_or(|wanted| wanted == *root))
                .filter_map(|(root, sessions)| {
                    (sessions.remove(session_id) && sessions.is_empty()).then(|| root.clone())
                })
                .collect();
            for root in &roots {
                shown_by.remove(root);
            }
            roots
        };
        let now = Instant::now();
        self.ending
            .lock()
            .extend(closing.iter().map(|root| (root.clone(), (session_id.to_string(), now))));
        for root in closing {
            self.send(WindowRequest::Done {
                session_id: session_id.to_string(),
                message: message.to_string(),
                workspace_root: Some(root),
            });
        }
    }

    /// A session other than the one that was done rejoins a window counting down (within the countdown): it stays
    /// open, and closes once that session is done too.
    fn joined(&self, workspace: &Path, session_id: &str) {
        if !self.background {
            return;
        }
        let key = workspace_key(workspace);
        {
            let mut ending = self.ending.lock();
            let Some((done_by, at)) = ending.get(&key) else {
                return;
            };
            if at.elapsed() > COUNTDOWN_WINDOW {
                ending.remove(&key);
                return;
            }
            if done_by == session_id {
                return;
            }
            ending.remove(&key);
        }
        self.shown_by.lock().entry(key).or_default().insert(session_id.to_string());
        self.send(WindowRequest::Rejoined {
            workspace: review_store::real_path(workspace),
        });
    }
}

/// How long after a session is done another one can rejoin its window: the countdown, and a little slack.
const COUNTDOWN_WINDOW: Duration = Duration::from_secs(CLOSE_AFTER_SECONDS as u64 + 2);

pub(super) fn handle(core: &ReviewCore, app_data: &AppData, request: WindowRequest) {
    // Opening pages and panels reads signals; the request effect must only track the requests.
    untrack(|| match request {
        WindowRequest::Show {
            review_id: _,
            workspace,
            pages,
        } => show_pages(core, app_data, pages, workspace),
        WindowRequest::Done {
            session_id,
            message,
            workspace_root,
        } => {
            tracing::info!(
                "agent session {session_id} is done with {}: {message}",
                workspace_root.as_deref().unwrap_or("all its windows")
            );
            let workspace = workspace_root
                .as_deref()
                .and_then(review_store::file_uri_to_path)
                .map(PathBuf::from);
            if let Some((_, _, window_tab)) =
                workspace.and_then(|workspace| find_workspace(app_data, &workspace))
            {
                close_soon(&window_tab, message);
            }
        }
        WindowRequest::Rejoined { workspace } => {
            if let Some((_, _, window_tab)) = find_workspace(app_data, &workspace)
                && window_tab.review.closing.get_untracked().is_some()
            {
                tracing::info!("the window of {} stays open: an agent rejoined its review", workspace.display());
                window_tab.review.closing.set(None);
            }
        }
    });
}

/// The pages showing a review opens, the last in front: a change review's overview and its pull-request page; a
/// review directory without patches (a design, a knowledge bundle) its document, which is its front page, titled
/// after it (the review's title is the document's); any other
/// review its overview. Reads the review directory, so it runs off the UI thread.
fn pages_to_show(review_id: &str, review: Option<&review_model::Review>) -> Vec<ReviewPage> {
    let overview = ReviewPage::overview(review_id);
    let Some(review) = review else {
        return vec![overview];
    };
    if super::pull_request::has_patches(review) {
        return vec![overview, ReviewPage::pull_request(review_id)];
    }
    review
        .bundle
        .as_ref()
        .and_then(|bundle| review_bundle::service::document_of(Path::new(&bundle.dir)))
        .map(|document| vec![ReviewPage::document(review_id, &document).titled(review.title.clone())])
        .unwrap_or_else(|| vec![overview])
}

/// Opens or focuses the window of `workspace` (a folder, opened as a local workspace) and opens `pages` in it, the
/// last in front.
fn show_pages(
    core: &ReviewCore,
    app_data: &AppData,
    pages: Vec<ReviewPage>,
    workspace: PathBuf,
) {
    let Some(page) = pages.first().cloned() else {
        return;
    };
    match find_workspace(app_data, &workspace) {
        Some((window, index, window_tab)) => {
            tracing::info!(
                "showing review {} in the window of {}",
                page.review_id,
                workspace.display()
            );
            window.active.set(index);
            // Shown again (another agent session): a countdown from an agent that was done with it stops.
            if window_tab.review.closing.get_untracked().is_some() {
                tracing::info!("the window of {} stays open: a review is shown in it", workspace.display());
                window_tab.review.closing.set(None);
            }
            for page in pages {
                reveal_page(&window_tab, page);
            }
            focus(&window);
        }
        None => {
            tracing::info!(
                "showing review {} in a new window of {}",
                page.review_id,
                workspace.display()
            );
            // The page opens once the new window's workspace appears (see `review::connect`).
            core.pending_pages
                .borrow_mut()
                .insert(workspace.clone(), pages);
            app_data.new_window(Some(workspace));
        }
    }
}

/// Opens or focuses the window of a folder, for launches handed over by the single-instance socket.
pub fn open_or_focus_folder(app_data: &AppData, folder: PathBuf) {
    let folder = review_store::real_path(&folder);
    match find_workspace(app_data, &folder) {
        Some((window, index, _)) => {
            tracing::info!("focusing the window of {}", folder.display());
            window.active.set(index);
            focus(&window);
        }
        None => {
            tracing::info!("opening a window for {}", folder.display());
            app_data.new_window(Some(folder));
        }
    }
}

/// The window, workspace tab index and workspace tab showing the local folder `workspace` (a resolved path).
fn find_workspace(
    app_data: &AppData,
    workspace: &Path,
) -> Option<(WindowData, usize, Rc<WindowTabData>)> {
    app_data
        .windows
        .get_untracked()
        .values()
        .find_map(|window| {
            window
                .window_tabs
                .get_untracked()
                .into_iter()
                .enumerate()
                .find(|(_, (_, window_tab))| {
                    window_tab.review.root.as_deref() == Some(workspace)
                })
                .map(|(index, (_, window_tab))| (window.clone(), index, window_tab))
        })
}

/// Brings a window to the front: un-minimized, ordered front, and the app activated.
fn focus(window: &WindowData) {
    window.window_id.minimized(false);
    window
        .common
        .app_view_id
        .get_untracked()
        .window_visible(true);
    floem::action::focus_window();
}

/// A workspace as [`AppWindows`] keys it: resolved through symbolic links, as a file URI.
fn workspace_key(workspace: &Path) -> String {
    review_store::path_to_file_uri(&review_store::real_path(workspace).to_string_lossy())
}

/// How long a window an agent is done with stays open.
const CLOSE_AFTER_SECONDS: u32 = 10;

/// Says why the window closes (`message`) in the Review panel, and closes it after a countdown unless the reviewer
/// clicks **Keep open**.
fn close_soon(window_tab: &WindowTabData, message: String) {
    let closing = window_tab.review.closing;
    tracing::info!(
        "closing the window of {} in {CLOSE_AFTER_SECONDS} s unless kept open: {message}",
        window_tab.review.root.as_deref().map(Path::display).map(|path| path.to_string()).unwrap_or_default()
    );
    closing.set(Some(super::workspace::ClosingNotice {
        message,
        seconds: CLOSE_AFTER_SECONDS,
    }));
    window_tab.show_panel(crate::panel::kind::PanelKind::Review);
    let close = close_now_action(window_tab);
    tick(closing, Rc::new(close));
}

fn tick(closing: floem::reactive::RwSignal<Option<super::workspace::ClosingNotice>>, close: Rc<dyn Fn()>) {
    floem::action::exec_after(std::time::Duration::from_secs(1), move |_| {
        // Kept open, or the window was closed meanwhile (its scope is disposed: stop, don't panic).
        let Some(notice) = closing.try_get_untracked().flatten() else {
            return;
        };
        if notice.seconds <= 1 {
            close();
            return;
        }
        closing.set(Some(super::workspace::ClosingNotice {
            seconds: notice.seconds - 1,
            ..notice
        }));
        tick(closing, close);
    });
}

/// **Keep open**: the countdown stops; closing the window later is the reviewer's.
pub fn keep_open_action(window_tab: &WindowTabData) -> impl Fn() + 'static {
    let closing = window_tab.review.closing;
    move || closing.set(None)
}

/// **Close now**, or the end of the countdown: closes the window (its workspace tab when it has several).
pub fn close_now_action(window_tab: &WindowTabData) -> impl Fn() + 'static {
    let closing = window_tab.review.closing;
    let window_common = window_tab.common.window_common.clone();
    move || {
        closing.set(None);
        tracing::info!("closing the review window");
        let command = if window_common.num_window_tabs.get_untracked() <= 1 {
            crate::command::WindowCommand::CloseWindow
        } else {
            crate::command::WindowCommand::CloseWorkspaceTab { index: None }
        };
        window_common.window_command.send(command);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review(bundle_dir: Option<&Path>) -> review_model::Review {
        let mut review = serde_json::json!({
            "id": "r1", "title": "T", "workspaceRoot": "file:///w", "scope": { "kind": "repository" },
            "participants": [], "threads": [], "activity": [], "nextThreadNumber": 1,
            "createdAt": 0, "updatedAt": 0,
        });
        if let Some(dir) = bundle_dir {
            review["bundle"] = serde_json::json!({ "dir": dir.to_string_lossy() });
        }
        serde_json::from_value(review).unwrap()
    }

    #[test]
    fn a_design_opens_its_document_and_a_change_its_pull_request() {
        let scratch = tempfile::tempdir().unwrap();
        let design = scratch.path().join("design");
        std::fs::create_dir_all(&design).unwrap();
        std::fs::write(design.join("index.md"), "# Design\n").unwrap();
        let pages = pages_to_show("r1", Some(&review(Some(&design))));
        assert_eq!(pages, vec![ReviewPage::document("r1", &design.join("index.md"))]);
        assert_eq!(pages[0].title.as_deref(), Some("T"));

        let change = scratch.path().join("change");
        std::fs::create_dir_all(&change).unwrap();
        std::fs::write(change.join("change.patch"), "diff --git a/x b/x\n").unwrap();
        assert_eq!(
            pages_to_show("r1", Some(&review(Some(&change)))),
            vec![ReviewPage::overview("r1"), ReviewPage::pull_request("r1")]
        );

        assert_eq!(pages_to_show("r1", Some(&review(None))), vec![ReviewPage::overview("r1")]);
        assert_eq!(pages_to_show("r1", None), vec![ReviewPage::overview("r1")]);
    }

    #[test]
    fn another_session_rejoining_a_window_counting_down_keeps_it_open() {
        let scratch = tempfile::tempdir().unwrap();
        let store = Arc::new(ReviewStore::with_home(scratch.path().join("home")));
        let (workspace, other) = (scratch.path().join("a"), scratch.path().join("b"));
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let (sender, requests) = mpsc::channel();
        let windows = AppWindows::new(sender, store, true);
        let next = || requests.try_recv().ok();
        windows.show("r1", &workspace, "s1");
        windows.show("r2", &other, "s9");
        assert!(matches!(next(), Some(WindowRequest::Show { .. })));
        assert!(matches!(next(), Some(WindowRequest::Show { .. })));
        // Nothing counts down yet: using the review changes nothing.
        windows.joined(&workspace, "s2");
        assert!(next().is_none());

        // s1's session ends: its window counts down (the other window doesn't).
        windows.done("s1", "ended", None);
        assert!(matches!(next(), Some(WindowRequest::Done { .. })));
        assert!(next().is_none());
        // The session that was done using the review again (e.g. get_review after approving) changes nothing.
        windows.joined(&workspace, "s1");
        assert!(next().is_none());
        // Another session uses it within the countdown: the window stays open...
        windows.joined(&workspace, "s2");
        match next() {
            Some(WindowRequest::Rejoined { workspace: rejoined }) => {
                assert_eq!(rejoined, review_store::real_path(&workspace))
            }
            other => panic!("{other:?}"),
        }
        windows.joined(&workspace, "s3");
        assert!(next().is_none());
        // ...and closes once that session is done too.
        windows.done("s2", "ended", None);
        assert!(matches!(next(), Some(WindowRequest::Done { .. })));

        // Not started in the background: windows never close for agents.
        let (sender, requests) = mpsc::channel();
        let windows = AppWindows::new(sender, Arc::new(ReviewStore::with_home(scratch.path().join("home"))), false);
        windows.show("r1", &workspace, "s1");
        windows.done("s1", "ended", None);
        windows.joined(&workspace, "s2");
        assert!(matches!(requests.try_recv(), Ok(WindowRequest::Show { .. })));
        assert!(requests.try_recv().is_err());
    }
}
