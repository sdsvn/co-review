//! Co-Review inside Lapce: the review core (store, MCP server, review crates) started with the app, and the review
//! UI (the Review panel, review pages in editor tabs, review windows).
//!
//! The review crates are headless; this module adapts them to Floem. Store changes arrive on the UI thread as the
//! [`ReviewCore::changes`] signal, and the MCP server's window requests ([`windows::WindowRequest`]) are handled by
//! [`connect`]. Lapce's own files only call in here through hooks marked `// co-review:`
//! (`docs/co-review/fork-surface.md`).

mod page;
mod panel;
mod windows;
mod workspace;

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, mpsc},
};

use floem::{
    ext_event::create_signal_from_channel,
    reactive::{
        ReadSignal, SignalGet, SignalWith, create_effect, provide_context,
        use_context,
    },
};
pub use page::{
    ReviewPage, ReviewPageId, ReviewPageKind, review_page_view,
    review_page_view_info,
};
pub use panel::review_panel;
use review_mcp::{CoReviewer, ServerHandle, ServerOptions, Services};
use review_store::{ReviewChange, ReviewStore};
use sha2::{Digest, Sha256};
pub use windows::{WindowRequest, open_or_focus_folder};
pub use workspace::ReviewWorkspaceData;

use crate::{app::AppData, panel::kind::PanelKind, window_tab::WindowTabData};

/// What the review UI shares, one per app: provided as a Floem context ([`ReviewCore::get`]).
pub struct ReviewCore {
    /// The store and the review crates, the same instances the MCP server uses.
    pub services: Services,
    /// The latest store change, on the UI thread. Every change is delivered, in order; read it in an effect.
    pub changes: ReadSignal<Option<ReviewChange>>,
    /// Started with `--background`: no window until a review is shown or the app is activated.
    pub background: bool,
    server: RefCell<Option<ServerHandle>>,
    reviewer: Option<Arc<CoReviewer>>,
    requests: ReadSignal<Option<WindowRequest>>,
    /// Pages to open once the window of their workspace (a resolved path) exists.
    pending_pages: RefCell<HashMap<PathBuf, ReviewPage>>,
    /// The workspaces (resolved paths) that have a window, as last told to the MCP server.
    open_workspaces: RefCell<HashSet<PathBuf>>,
}

impl ReviewCore {
    /// Starts the review core and the MCP server (recorded in `$CO_REVIEW_HOME/server.json`), and provides the core
    /// as a context. `default_root` is the repository the app was started with, the default `root` of MCP sessions.
    pub fn start(default_root: Option<PathBuf>, background: bool) -> Rc<ReviewCore> {
        let home = review_store::co_review_home();
        let store = Arc::new(ReviewStore::with_home(home.clone()));
        let (request_sender, request_receiver) = mpsc::channel();
        let services = Services::new(
            Arc::clone(&store),
            Arc::new(windows::AppWindows::new(request_sender)),
        );
        let options = ServerOptions {
            port: std::env::var("CO_REVIEW_PORT")
                .ok()
                .and_then(|port| port.trim().parse().ok())
                .unwrap_or(0),
            default_root,
            ..ServerOptions::default()
        };
        let server = match review_mcp::start(options, services.clone()) {
            Ok(server) => {
                tracing::info!("Co-Review MCP server listening at {}", server.url());
                Some(server)
            }
            Err(error) => {
                tracing::error!("the Co-Review MCP server did not start: {error:#}");
                None
            }
        };
        record_app(&home);
        let core = Rc::new(ReviewCore {
            reviewer: server.as_ref().map(|server| Arc::clone(server.reviewer())),
            server: RefCell::new(server),
            changes: store_changes(&store),
            requests: create_signal_from_channel(request_receiver),
            services,
            background,
            pending_pages: RefCell::default(),
            open_workspaces: RefCell::default(),
        });
        provide_context(Rc::clone(&core));
        core
    }

    /// The app's review core; `None` only before [`ReviewCore::start`].
    pub fn get() -> Option<Rc<ReviewCore>> {
        use_context::<Rc<ReviewCore>>()
    }

    /// The app is quitting: the agents of every open window hear that it closed, and the server stops (removing its
    /// `server.json` record).
    pub fn shut_down(&self) {
        if let Some(reviewer) = &self.reviewer {
            for workspace in self.open_workspaces.borrow_mut().drain() {
                reviewer.window_closed(&workspace_uri(&workspace));
            }
        }
        if let Some(server) = self.server.borrow_mut().take() {
            server.stop();
        }
    }

    /// Tells the MCP server which workspaces gained or lost their window, off the UI thread (it reads and writes
    /// reviews).
    fn workspaces_changed(&self, workspaces: HashSet<PathBuf>) {
        let (opened, closed): (Vec<PathBuf>, Vec<PathBuf>) = {
            let mut open = self.open_workspaces.borrow_mut();
            let opened = workspaces.difference(&open).cloned().collect();
            let closed = open.difference(&workspaces).cloned().collect();
            *open = workspaces;
            (opened, closed)
        };
        let Some(reviewer) = self.reviewer.clone() else {
            return;
        };
        if opened.is_empty() && closed.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            for workspace in opened {
                reviewer.window_opened(&workspace_uri(&workspace));
            }
            for workspace in closed {
                reviewer.window_closed(&workspace_uri(&workspace));
            }
        });
    }
}

/// Connects the review core to the app's windows: the MCP server's window requests are served, and the server hears
/// when a workspace's window opens or closes.
pub fn connect(core: Rc<ReviewCore>, app_data: AppData) {
    {
        let (core, app_data) = (Rc::clone(&core), app_data.clone());
        let requests = core.requests;
        create_effect(move |_| {
            if let Some(request) = requests.get() {
                windows::handle(&core, &app_data, request);
            }
        });
    }
    let windows = app_data.windows;
    create_effect(move |_| {
        let window_tabs: Vec<Rc<WindowTabData>> = windows.with(|windows| {
            windows
                .values()
                .flat_map(|window| window.window_tabs.get())
                .map(|(_, window_tab)| window_tab)
                .collect()
        });
        let mut workspaces = HashSet::new();
        for window_tab in window_tabs {
            let Some(root) = window_tab.review.root.clone() else {
                continue;
            };
            let pending = core.pending_pages.borrow_mut().remove(&root);
            if let Some(page) = pending {
                reveal_page(&window_tab, page);
            }
            workspaces.insert(root);
        }
        core.workspaces_changed(workspaces);
    });
}

/// Opens `page` in the window tab and shows the Review panel beside it.
pub fn reveal_page(window_tab: &WindowTabData, page: ReviewPage) {
    window_tab.show_panel(PanelKind::Review);
    window_tab.main_split.open_review_page(page);
}

/// "Review: Open Review Overview": the overview of the workspace's most recently updated open review, or the Review
/// panel (which says there is none) when it has none.
pub fn open_overview(window_tab: &WindowTabData) {
    let latest = window_tab.review.open_reviews().into_iter().next();
    match latest {
        Some(review) => reveal_page(window_tab, ReviewPage::overview(&review.id)),
        None => window_tab.show_panel(PanelKind::Review),
    }
}

/// The single-instance socket: Lapce's own, or one per `$CO_REVIEW_HOME` when it is set, so apps on different homes
/// (tests, agents) never hand their launches to each other.
pub fn local_socket(lapce_socket: Option<PathBuf>) -> Option<PathBuf> {
    let home = std::env::var_os("CO_REVIEW_HOME").filter(|home| !home.is_empty());
    match (lapce_socket, home) {
        (Some(socket), Some(home)) => {
            let hash = format!("{:x}", Sha256::digest(home.as_encoded_bytes()));
            // Short: a Unix socket path has at most 104 bytes on macOS.
            let name = format!("cr-{}.sock", hash.get(..8).unwrap_or(&hash));
            Some(socket.with_file_name(name))
        }
        (socket, _) => socket,
    }
}

/// A workspace path as the store keys it: resolved through symbolic links, as a file URI.
fn workspace_uri(workspace: &Path) -> String {
    review_store::path_to_file_uri(&workspace.to_string_lossy())
}

/// Store changes as a signal: the store broadcasts on a smol channel, Floem reads std channels.
fn store_changes(store: &ReviewStore) -> ReadSignal<Option<ReviewChange>> {
    let changes = store.subscribe_changes();
    let (sender, receiver) = mpsc::channel();
    let forwarded = std::thread::Builder::new()
        .name("ReviewStoreChanges".to_owned())
        .spawn(move || {
            while let Ok(change) = changes.recv_blocking() {
                if sender.send(change).is_err() {
                    break;
                }
            }
        });
    if let Err(error) = forwarded {
        tracing::error!("review changes will not reach the window: {error}");
    }
    create_signal_from_channel(receiver)
}

/// `logs/app.json`: this app's process and version, for `co-review` to recognise a stale or stuck app.
fn record_app(home: &Path) {
    let logs = home.join("logs");
    let record = serde_json::json!({
        "pid": std::process::id(),
        "version": env!("CARGO_PKG_VERSION"),
        "started": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });
    let written = std::fs::create_dir_all(&logs)
        .and_then(|()| std::fs::write(logs.join("app.json"), record.to_string()));
    if let Err(error) = written {
        tracing::error!("could not record the app in {}: {error}", logs.display());
    }
}
