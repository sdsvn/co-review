//! The app's windows for the MCP server ([`review_mcp::ReviewWindows`]): showing a review opens its workspace's
//! window, or focuses the one already open, with the review's overview page in front (a change review's pull-request
//! page in front of it).

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, mpsc},
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
    /// Show `review_id` in the window of `workspace` (resolved through symbolic links); a change review's
    /// pull-request page in front of its overview.
    Show {
        review_id: String,
        workspace: PathBuf,
        pull_request: bool,
    },
    /// The agent session is done with the window of `workspace_root` (a file URI), or with all of its windows.
    Done {
        session_id: String,
        message: String,
        workspace_root: Option<String>,
    },
}

/// Called on the MCP server's threads: forwards each request to the UI thread.
pub(super) struct AppWindows {
    requests: mpsc::Sender<WindowRequest>,
    store: Arc<ReviewStore>,
    /// Started in the background (by an agent): windows agents showed close when those agents are done.
    background: bool,
    /// Workspace (file URI) → the agent sessions that showed its window, in a background start.
    shown_by: Mutex<HashMap<String, HashSet<String>>>,
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
            self.shown_by
                .lock()
                .entry(workspace_key(workspace))
                .or_default()
                .insert(session_id.to_string());
        }
        // Read here, off the UI thread: whether the review has patches to show as a pull request.
        let pull_request = self
            .store
            .get(review_id)
            .is_some_and(|review| super::pull_request::has_patches(&review));
        self.send(WindowRequest::Show {
            review_id: review_id.to_string(),
            workspace: review_store::real_path(workspace),
            pull_request,
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
        for root in closing {
            self.send(WindowRequest::Done {
                session_id: session_id.to_string(),
                message: message.to_string(),
                workspace_root: Some(root),
            });
        }
    }
}

pub(super) fn handle(core: &ReviewCore, app_data: &AppData, request: WindowRequest) {
    // Opening pages and panels reads signals; the request effect must only track the requests.
    untrack(|| match request {
        WindowRequest::Show {
            review_id,
            workspace,
            pull_request,
        } => {
            let mut pages = vec![ReviewPage::overview(&review_id)];
            if pull_request {
                pages.push(ReviewPage::pull_request(&review_id));
            }
            show_pages(core, app_data, pages, workspace)
        }
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
    });
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
        let Some(notice) = closing.get_untracked() else {
            // Kept open.
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
