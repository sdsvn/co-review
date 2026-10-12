//! What the MCP tools work with: the store, the review crates (directories and anchor tracking from
//! `review_bundle`, code folders from `review_code`, overview pages from `review_pages`, the repo map from
//! `review_index`), and what the app provides: its windows, and Tree-sitter until `review_syntax` exists.

use std::path::Path;
use std::sync::Arc;

use review_bundle::BundleService;
use review_code::CodeFolders;
use review_github::{DefaultGhRunner, GhRunner};
use review_model::{Range, SyntaxAnchor};
use review_pages::{OverviewPages, RepositorySections};
use review_store::ReviewStore;

use crate::coordination::{AgentPresenceTracker, HumanDecisions};

/// The note `open_review` returns once the review is shown in a window.
pub const SHOWN_NOTE: &str = "Shown in the Co-Review desktop app.";

/// The app's review windows. The app also tells the server when the reviewer closes or reopens one
/// ([`crate::CoReviewer::window_closed`], [`crate::CoReviewer::window_opened`]).
pub trait ReviewWindows: Send + Sync {
    /// Shows `workspace` (a repository, review directory or code folder) with `review_id` in front: opens its window,
    /// or focuses it. `session_id` is the agent session asking, for [`ReviewWindows::done`]. Returns the note for the
    /// agent.
    fn show(&self, review_id: &str, workspace: &Path, session_id: &str) -> String;

    /// The agent session is done with the window of `workspace_root` (a file URI), or with all its windows when
    /// `None`: a window only that session showed may close. `message` tells the reviewer why.
    fn done(&self, session_id: &str, message: &str, workspace_root: Option<&str>);
}

/// No app: shows nothing, and returns the desktop note.
struct HeadlessWindows;

impl ReviewWindows for HeadlessWindows {
    fn show(&self, _review_id: &str, _workspace: &Path, _session_id: &str) -> String {
        SHOWN_NOTE.to_string()
    }

    fn done(&self, _session_id: &str, _message: &str, _workspace_root: Option<&str>) {}
}

/// Tree-sitter for a finding's location (`SyntaxService` in the TypeScript): its syntax anchor and the symbol
/// around it.
pub trait CodeSyntax: Send + Sync {
    fn capture_anchor(&self, language: &str, text: &str, range: &Range) -> Option<SyntaxAnchor>;
    /// The dotted path of the innermost symbol containing `line` (0-based), e.g. `OrderService.CreateOrder`.
    fn enclosing_symbol(&self, language: &str, text: &str, line: u32) -> Option<String>;
}

/// Until `review_syntax` exists: findings are anchored by their text, without a symbol.
struct NoSyntax;

impl CodeSyntax for NoSyntax {
    fn capture_anchor(&self, _: &str, _: &str, _: &Range) -> Option<SyntaxAnchor> {
        None
    }

    fn enclosing_symbol(&self, _: &str, _: &str, _: u32) -> Option<String> {
        None
    }
}

/// Everything the MCP server works with. The app shares the same instances with its UI (one bundle service, one set
/// of code folders, one set of overview pages per store).
#[derive(Clone)]
pub struct Services {
    pub store: Arc<ReviewStore>,
    pub windows: Arc<dyn ReviewWindows>,
    /// Review directories (`open_review({ dir })`, inline content), findings on them, and anchor tracking.
    pub bundles: Arc<BundleService>,
    pub code_folders: Arc<CodeFolders>,
    pub pages: Arc<OverviewPages>,
    pub syntax: Arc<dyn CodeSyntax>,
    /// Runs `gh` for `post_review_to_github`.
    pub github: Arc<dyn GhRunner + Send + Sync>,
    pub decisions: Arc<HumanDecisions>,
    pub presence: Arc<AgentPresenceTracker>,
}

impl Services {
    /// The review crates over `store`, showing reviews through `windows`.
    pub fn new(store: Arc<ReviewStore>, windows: Arc<dyn ReviewWindows>) -> Self {
        Self {
            bundles: Arc::new(BundleService::new(Arc::clone(&store))),
            code_folders: Arc::new(CodeFolders::new(Arc::clone(&store))),
            pages: OverviewPages::new(Arc::clone(&store), Arc::new(RepositorySections)),
            store,
            windows,
            syntax: Arc::new(NoSyntax),
            github: Arc::new(DefaultGhRunner),
            decisions: Arc::default(),
            presence: Arc::default(),
        }
    }

    /// Without the app: [`HeadlessWindows`].
    pub fn headless(store: Arc<ReviewStore>) -> Self {
        Self::new(store, Arc::new(HeadlessWindows))
    }
}

/// The document of a review directory, by the review-directory rules (`index.markdown` first, …).
pub fn document_of(dir: &str) -> Option<String> {
    review_bundle::service::document_of(Path::new(dir))
        .map(|file| file.to_string_lossy().into_owned())
}
