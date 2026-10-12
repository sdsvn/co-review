//! The app's windows for the MCP server ([`review_mcp::ReviewWindows`]): showing a review opens its workspace's
//! window, or focuses the one already open, with the review's overview page in front.

use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc,
};

use floem::{
    WindowIdExt,
    reactive::{SignalGet, SignalUpdate, untrack},
};
use review_mcp::{ReviewWindows, SHOWN_NOTE};

use super::{ReviewCore, ReviewPage, reveal_page};
use crate::{app::AppData, window::WindowData, window_tab::WindowTabData};

/// What the MCP server asks of the app's windows, handled on the UI thread.
#[derive(Clone, Debug)]
pub enum WindowRequest {
    /// Show `review_id` in the window of `workspace` (resolved through symbolic links).
    Show {
        review_id: String,
        workspace: PathBuf,
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
}

impl AppWindows {
    pub(super) fn new(requests: mpsc::Sender<WindowRequest>) -> Self {
        Self { requests }
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
    fn show(&self, review_id: &str, workspace: &Path, _session_id: &str) -> String {
        self.send(WindowRequest::Show {
            review_id: review_id.to_string(),
            workspace: review_store::real_path(workspace),
        });
        SHOWN_NOTE.to_string()
    }

    fn done(&self, session_id: &str, message: &str, workspace_root: Option<&str>) {
        self.send(WindowRequest::Done {
            session_id: session_id.to_string(),
            message: message.to_string(),
            workspace_root: workspace_root.map(str::to_string),
        });
    }
}

pub(super) fn handle(core: &ReviewCore, app_data: &AppData, request: WindowRequest) {
    // Opening pages and panels reads signals; the request effect must only track the requests.
    untrack(|| match request {
        WindowRequest::Show {
            review_id,
            workspace,
        } => show(core, app_data, ReviewPage::overview(&review_id), workspace),
        WindowRequest::Done {
            session_id,
            message,
            workspace_root,
        } => {
            tracing::info!(
                "agent session {session_id} is done with {}: {message}",
                workspace_root.as_deref().unwrap_or("all its windows")
            );
        }
    });
}

/// Opens or focuses the window of `workspace` (a folder, opened as a local workspace) and opens `page` in it.
pub fn show(
    core: &ReviewCore,
    app_data: &AppData,
    page: ReviewPage,
    workspace: PathBuf,
) {
    match find_workspace(app_data, &workspace) {
        Some((window, index, window_tab)) => {
            tracing::info!(
                "showing review {} in the window of {}",
                page.review_id,
                workspace.display()
            );
            window.active.set(index);
            reveal_page(&window_tab, page);
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
                .insert(workspace.clone(), page);
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
