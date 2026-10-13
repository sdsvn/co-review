//! Co-Review inside Lapce: the review core (store, MCP server, review crates) started with the app, and the review
//! UI (the Review panel, review pages in editor tabs, review windows).
//!
//! The review crates are headless; this module adapts them to Floem. Store changes arrive on the UI thread as the
//! [`ReviewCore::changes`] signal, and the MCP server's window requests ([`windows::WindowRequest`]) are handled by
//! [`connect`]. Lapce's own files only call in here through hooks marked `// co-review:`
//! (`docs/co-review/fork-surface.md`).

pub mod changed_files;
pub mod code;
pub mod commands;
mod design_tree;
pub mod document;
mod markdown_view;
mod mermaid;
mod page;
mod panel;
pub mod picker;
pub mod setup;
pub mod pull_request;
mod stale;
mod submit;
pub mod theme;
pub mod drafts;
pub mod inline;
mod selection_bar;
pub mod thread;
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
        ReadSignal, RwSignal, SignalGet, SignalUpdate, SignalWith,
        create_effect, create_rw_signal, provide_context,
        use_context,
    },
};
pub use page::{
    ReviewPage, ReviewPageId, ReviewPageKind, review_page_view,
    review_page_view_info,
};
pub use panel::review_panel;
use review_agents::AgentService;
use review_mcp::{CoReviewer, ServerHandle, ServerOptions, Services};
use review_store::{ReviewChange, ReviewStore};
use sha2::{Digest, Sha256};
pub use changed_files::open_changed_file;
pub use windows::{WindowRequest, open_or_focus_folder};
pub use workspace::ReviewWorkspaceData;

use crate::{app::AppData, panel::kind::PanelKind, window_tab::WindowTabData};

/// What the review UI shares, one per app: provided as a Floem context ([`ReviewCore::get`]).
pub struct ReviewCore {
    /// The store and the review crates, the same instances the MCP server uses.
    pub services: Services,
    /// The ACP agents of every review (Connect agent…, questions in threads, Stop), sharing the MCP server's
    /// decisions so permission requests are answered like `ask_reviewer` questions.
    pub agents: Arc<AgentService>,
    /// Whether each review's MCP agent is waiting for the reviewer (`await_reviewer`), by review id.
    pub listening: RwSignal<im::HashMap<String, bool>>,
    /// The latest store change, on the UI thread. Every change is delivered, in order; read it in an effect.
    pub changes: ReadSignal<Option<ReviewChange>>,
    /// Started with `--background`: no window until a review is shown or the app is activated.
    pub background: bool,
    /// The version an update installed over this app (`stale.rs`): the app quits once no window is open.
    stale: RwSignal<Option<String>>,
    stale_reports: ReadSignal<Option<String>>,
    server: RefCell<Option<ServerHandle>>,
    reviewer: Option<Arc<CoReviewer>>,
    requests: ReadSignal<Option<WindowRequest>>,
    /// Pages to open once the window of their workspace (a resolved path) exists.
    pending_pages: RefCell<HashMap<PathBuf, Vec<ReviewPage>>>,
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
            Arc::new(windows::AppWindows::new(
                request_sender,
                Arc::clone(&store),
                background,
            )),
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
        let agents = AgentService::new(
            Arc::clone(&services.store),
            Arc::clone(&services.decisions),
        );
        if let Err(error) = agents.watch_store() {
            tracing::error!("ACP agents will not follow review changes: {error:#}");
        }
        let listening = presence_signal(&services);
        let (stale_sender, stale_receiver) = mpsc::channel();
        if background {
            stale::watch(stale_sender);
        }
        stale::quit_on_terminate();
        let core = Rc::new(ReviewCore {
            agents,
            listening,
            reviewer: server.as_ref().map(|server| Arc::clone(server.reviewer())),
            server: RefCell::new(server),
            changes: store_changes(&store),
            requests: create_signal_from_channel(request_receiver),
            services,
            background,
            stale: create_rw_signal(None),
            stale_reports: create_signal_from_channel(stale_receiver),
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
        self.agents.shutdown();
    }

    /// Tells the MCP server which workspaces gained or lost their window, off the UI thread (it reads and writes
    /// reviews).
    /// `wait`: the app is about to quit, so the agents must have heard before this returns.
    fn workspaces_changed(&self, workspaces: HashSet<PathBuf>, wait: bool) {
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
        let told = std::thread::spawn(move || {
            for workspace in opened {
                reviewer.window_opened(&workspace_uri(&workspace));
            }
            for workspace in closed {
                reviewer.window_closed(&workspace_uri(&workspace));
            }
        });
        if wait && told.join().is_err() {
            tracing::error!("telling agents their review windows closed failed");
        }
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
    {
        let (stale, reports) = (core.stale, core.stale_reports);
        create_effect(move |_| {
            if let Some(on_disk) = reports.get() {
                stale.set(Some(on_disk));
            }
        });
        // Replaced on disk: quit now if no window is open, else once the last one closes (never under a reviewer).
        create_effect(move |_| {
            let Some(on_disk) = stale.get() else {
                return;
            };
            if windows.with(|windows| windows.is_empty()) {
                tracing::error!(
                    "Co-Review {} was replaced on disk by {on_disk}: quitting, so the next launch starts it",
                    env!("CARGO_PKG_VERSION")
                );
                floem::quit_app();
            } else {
                tracing::info!(
                    "Co-Review {on_disk} is installed: this app ({}) quits once its windows are closed",
                    env!("CARGO_PKG_VERSION")
                );
            }
        });
    }
    create_effect(move |had_windows: Option<bool>| {
        let has_windows = windows.with(|windows| !windows.is_empty());
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
            // The workspace's unsent comments, kept across restarts (once per window tab).
            floem::reactive::untrack(|| drafts::keep(&window_tab));
            // A review an agent showed opens now; otherwise the window opens on its latest review once loaded.
            let pending = core.pending_pages.borrow_mut().remove(&root);
            floem::reactive::untrack(|| match pending {
                Some(pages) => show_review(&window_tab, pages),
                None => open_on_review(&window_tab),
            });
            workspaces.insert(root);
        }
        // Started in the background (by an agent): with no window left, the app quits.
        let quit = core.background && had_windows == Some(true) && !has_windows;
        core.workspaces_changed(workspaces, quit);
        if quit {
            tracing::info!("the last window closed; quitting (started in the background)");
            floem::quit_app();
        }
        has_windows
    });
}

/// The app was activated (`co-review:` hook on `AppEvent::Reopen`): an app an update replaced on disk quits instead
/// (`stale.rs`, checked off the UI thread); otherwise `open` runs.
pub fn activated(open: impl FnOnce() + 'static) {
    let Some(core) = ReviewCore::get() else {
        open();
        return;
    };
    let stale = core.stale;
    let send = floem::ext_event::create_ext_action(floem::reactive::Scope::new(), move |on_disk: Option<String>| {
        match on_disk {
            Some(on_disk) => stale.set(Some(on_disk)),
            None => open(),
        }
    });
    let checking = std::thread::Builder::new()
        .name("ReviewStaleVersion".to_owned())
        .spawn(move || send(stale::check()));
    if let Err(error) = checking {
        tracing::error!("the version on disk was not checked: {error}");
    }
}

/// Opens `page` in the window tab and shows the Review panel beside it.
pub fn reveal_page(window_tab: &WindowTabData, page: ReviewPage) {
    window_tab.review.active.set(Some(page.review_id.clone()));
    window_tab.show_panel(PanelKind::Review);
    window_tab.main_split.open_review_page(page);
}

/// Puts a review in front: opens its `pages` with the first one the focused tab (opened after the editors Lapce
/// restored, so in front of them), shows the Review panel with the filter the panel picks, and moves the keyboard
/// focus to the page.
pub(crate) fn show_review(window_tab: &WindowTabData, pages: Vec<ReviewPage>) {
    let Some(front) = pages.first().cloned() else {
        return;
    };
    window_tab.review.shown.set(true);
    window_tab.review.panel_filter.set(None);
    for page in &pages {
        reveal_page(window_tab, page.clone());
    }
    if pages.len() > 1 {
        reveal_page(window_tab, front.clone());
    }
    window_tab.common.focus.set(crate::window_tab::Focus::Workbench);
    let in_front = window_tab.main_split.active_editor_tab.get_untracked().and_then(|id| {
        window_tab.main_split.editor_tabs.with_untracked(|tabs| {
            tabs.get(&id).and_then(|tab| {
                tab.with_untracked(|tab| match tab.children.get(tab.active) {
                    Some((_, _, crate::editor_tab::EditorTabChild::Review(_, page))) => {
                        Some((page.kind, tab.active, tab.children.len()))
                    }
                    _ => None,
                })
            })
        })
    });
    tracing::info!(
        "review {} in front in {}: pages {:?}, focused tab (page, index, tabs) {:?}",
        front.review_id,
        window_tab.review.root.as_deref().map(|root| root.display().to_string()).unwrap_or_default(),
        pages.iter().map(|page| page.kind).collect::<Vec<_>>(),
        in_front
    );
}

/// A window opening on a workspace with an open review (the reviewer's launch, Open Folder, a restored session) opens
/// on it, once its reviews are loaded: its front page in front of the restored editors, the panel beside it. Once per
/// window; a workspace without open reviews opens as it was. A review an agent shows meanwhile wins.
fn open_on_review(window_tab: &Rc<WindowTabData>) {
    if window_tab.review.opening.replace(true) || window_tab.review.root.is_none() {
        return;
    }
    let loaded = window_tab.review.loaded;
    let window_tab = Rc::clone(window_tab);
    let cx = window_tab.scope;
    cx.create_effect(move |done: Option<bool>| {
        if done == Some(true) || !loaded.get() {
            return done.unwrap_or(false);
        }
        floem::reactive::untrack(|| {
            let data = &window_tab.review;
            let reviews: Vec<Arc<review_model::Review>> =
                data.reviews.with_untracked(|reviews| reviews.iter().cloned().collect());
            let Some(review) = pick_review(data.active.get_untracked().as_deref(), &reviews) else {
                return;
            };
            if data.shown.get() {
                return;
            }
            let shown = Rc::clone(&data.shown);
            let window_tab = Rc::clone(&window_tab);
            let send = floem::ext_event::create_ext_action(cx, move |pages: Vec<ReviewPage>| {
                if !shown.get() {
                    show_review(&window_tab, pages);
                }
            });
            // The pages depend on the review directory's files: read off the UI thread.
            let read = std::thread::Builder::new()
                .name("ReviewFrontPages".to_owned())
                .spawn(move || send(windows::front_pages(&review.id, Some(&review))));
            if let Err(error) = read {
                tracing::error!("the window will not open on its review: {error}");
            }
        });
        true
    });
}

/// The review a window opens on: its active review when that is open (one an agent just showed), else the open one
/// last worked on; `None` when every review is archived or there is none.
fn pick_review(
    active: Option<&str>,
    reviews: &[Arc<review_model::Review>],
) -> Option<Arc<review_model::Review>> {
    reviews
        .iter()
        .find(|review| review.archived_at.is_none() && Some(review.id.as_str()) == active)
        .or_else(|| workspace::latest_open(reviews.iter()))
        .cloned()
}

/// "Review: Open Review Overview": the overview of the workspace's open review last worked on, or the Review
/// panel (which says there is none) when it has none.
pub fn open_overview(window_tab: &WindowTabData) {
    let reviews = window_tab.review.open_reviews();
    let latest = workspace::latest_open(reviews.iter()).cloned();
    match latest {
        Some(review) => reveal_page(window_tab, ReviewPage::overview(&review.id)),
        None => window_tab.show_panel(PanelKind::Review),
    }
}

/// Lapce's commands the app doesn't offer (palette, keybindings): remote development (SSH, WSL), since the app
/// ships no lapce-proxy for remote workspaces.
pub fn hides_workbench_command(command: &crate::command::LapceWorkbenchCommand) -> bool {
    use crate::command::LapceWorkbenchCommand::*;
    match command {
        ConnectSshHost | DisconnectRemote => true,
        #[cfg(windows)]
        ConnectWslHost => true,
        _ => false,
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

/// MCP agents' presence as a signal: the tracker broadcasts on a smol channel, Floem reads std channels.
fn presence_signal(services: &Services) -> RwSignal<im::HashMap<String, bool>> {
    let listening = create_rw_signal(im::HashMap::new());
    let presence = services.presence.subscribe();
    let (sender, receiver) = mpsc::channel();
    let forwarded = std::thread::Builder::new()
        .name("ReviewAgentPresence".to_owned())
        .spawn(move || {
            while let Ok(update) = presence.recv_blocking() {
                if sender.send(update).is_err() {
                    break;
                }
            }
        });
    if let Err(error) = forwarded {
        tracing::error!("agents' listening status will not reach the window: {error}");
    }
    let updates = create_signal_from_channel(receiver);
    create_effect(move |_| {
        if let Some(update) = updates.get() {
            listening.update(|listening| {
                listening.insert(update.review_id, update.listening);
            });
        }
    });
    listening
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    /// A review created at `created`, its file last written at `updated`.
    fn review(id: &str, archived: bool, created: u64, updated: u64) -> Arc<review_model::Review> {
        let mut review = serde_json::json!({
            "id": id, "title": "T", "workspaceRoot": "file:///w", "scope": { "kind": "repository" },
            "participants": [], "threads": [], "activity": [], "nextThreadNumber": 1,
            "createdAt": created, "updatedAt": updated,
        });
        if archived {
            review["archivedAt"] = serde_json::json!(1);
        }
        Arc::new(serde_json::from_value(review).unwrap())
    }

    #[test]
    fn a_window_opens_on_its_active_review_or_the_open_one_last_worked_on() {
        // "older" was written last only because its window closed (`closed`): "latest" was worked on last.
        let reviews = [
            review("older", false, 10, 90),
            review("archived", true, 50, 50),
            review("latest", false, 20, 30),
        ];
        let picked = |active| super::pick_review(active, &reviews).map(|review| review.id.clone());
        assert_eq!(picked(Some("older")).as_deref(), Some("older"));
        assert_eq!(picked(None).as_deref(), Some("latest"));
        assert_eq!(picked(Some("archived")).as_deref(), Some("latest"));
        assert_eq!(picked(Some("gone")).as_deref(), Some("latest"));
        assert!(super::pick_review(None, &[review("archived", true, 1, 1)]).is_none());
        assert!(super::pick_review(None, &[]).is_none());
    }

    #[test]
    fn remote_development_is_not_in_the_palette() {
        let commands = crate::command::lapce_internal_commands();
        for hidden in ["connect_ssh_host", "connect_wsl_host", "disconnect_remote"] {
            assert!(!commands.contains_key(hidden), "{hidden}");
        }
        assert!(commands.contains_key("new_window"));
    }
}
