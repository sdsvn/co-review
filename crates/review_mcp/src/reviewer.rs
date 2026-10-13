//! The co-reviewer tools, independent of the transport: an agent opens a review, then blocks on `await_reviewer`
//! until the reviewer asks something or submits, answers with `reply`, and can add findings or ask the reviewer to
//! decide. Port of `co-reviewer-mcp.ts`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use review_model::{
    AgentConfig, AgentState, AgentTransport, CodeAnchor, CodeLocation, LocationKind, MessageStatus,
    Participant, ParticipantKind, PermissionOption, PermissionRequest, Position, Range, Review,
    ReviewBundle, ReviewClosed, ReviewDecision, ReviewScope, ReviewThread, Severity, ThreadIntent,
    ThreadOptions, ThreadStatus, VerdictToAgent, waits_for_agent,
};
use review_pages::{RepositorySections, SectionBuilder as _};
use review_store::{CreateReviewParams, MessagePatch, ReviewChange};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::coordination::now_millis;
use crate::paths::{basename, is_within, path_to_file_uri, real_path, resolve, uri_to_path};
use crate::payloads::{
    Object, accepted_suggestions, code_root_of, comment_of, describe, find_thread, root_of,
    thread_ref, window_root_of,
};
use crate::prompts::ANSWER_STYLE_SHORT;
use crate::services::{Services, document_of};

/// What a tool returns: text (JSON for most tools), and whether it failed.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn json(value: &Value) -> Self {
        Self {
            text: serde_json::to_string_pretty(value).unwrap_or_default(),
            is_error: false,
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    pub fn fail(message: impl Into<String>) -> Self {
        Self {
            text: message.into(),
            is_error: true,
        }
    }
}

/// Sends a review event into the agent's session (Claude Code channels).
pub trait ChannelSink: Send + Sync {
    fn notify(&self, content: String, meta: Map<String, Value>);
}

/// One agent's MCP session: what it was handed, and which review it works on.
pub struct Session {
    pub id: String,
    reviewer: Arc<CoReviewer>,
    /// The repository the session was opened for (`?root=`), else the app's.
    root: Mutex<Option<String>>,
    client_name: Mutex<Option<String>>,
    state: Mutex<SessionState>,
    channel: Mutex<Option<Channel>>,
}

#[derive(Default)]
struct SessionState {
    /// Human message ids already handed to this agent, so each question is delivered once.
    delivered: HashSet<String>,
    /// Submissions already returned by await_reviewer.
    consumed_submit: u32,
    /// Threads handed to this agent and not answered yet (thread id → review id): given back if the session ends.
    working: HashMap<String, String>,
    /// The close (`Review.closed.at`) this session was told about, so it is told once.
    told_closed: Option<u64>,
    current_review_id: Option<String>,
}

struct Channel {
    sink: Arc<dyn ChannelSink>,
    task: Option<tokio::task::AbortHandle>,
}

impl Session {
    pub fn new(reviewer: Arc<CoReviewer>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            root: Mutex::new(reviewer.default_root.clone()),
            reviewer,
            client_name: Mutex::new(None),
            state: Mutex::new(SessionState::default()),
            channel: Mutex::new(None),
        }
    }

    pub fn reviewer(&self) -> &Arc<CoReviewer> {
        &self.reviewer
    }

    /// The handshake: the client's name and the repository it asked for (`?root=`).
    pub fn initialized(&self, client_name: &str, root: Option<&str>) {
        *self.client_name.lock() = Some(client_name.to_string());
        if let Some(root) = root {
            *self.root.lock() = Some(real_path(root));
        }
    }

    /// Where the agent's Claude Code channel goes; set once the session can send notifications.
    pub fn set_channel_sink(&self, sink: Arc<dyn ChannelSink>) {
        let mut channel = self.channel.lock();
        if channel.is_none() {
            *channel = Some(Channel { sink, task: None });
        }
    }

    pub fn agent(&self) -> Participant {
        let name = self
            .client_name
            .lock()
            .clone()
            .unwrap_or_else(|| "Agent".to_string());
        Participant {
            id: format!("agent:mcp:{name}"),
            kind: ParticipantKind::Agent,
            name,
        }
    }

    fn default_root(&self) -> Option<String> {
        self.root.lock().clone()
    }

    fn current_review_id(&self) -> Option<String> {
        self.state.lock().current_review_id.clone()
    }

    fn resolve_review(&self, review_id: Option<&str>, root: Option<&str>) -> Option<Review> {
        let store = &self.reviewer.services.store;
        if let Some(id) = review_id
            .map(str::to_string)
            .or_else(|| self.current_review_id())
        {
            return store.get(&id);
        }
        let workspace = root.map(str::to_string).or_else(|| self.default_root())?;
        store.list(&path_to_file_uri(&workspace)).into_iter().next()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(task) = self
            .channel
            .get_mut()
            .as_mut()
            .and_then(|channel| channel.task.take())
        {
            task.abort();
        }
        if self.client_name.get_mut().is_none() {
            return;
        }
        let name = self.agent().name;
        let services = &self.reviewer.services;
        services
            .windows
            .done(&self.id, &format!("{name} ended its session."), None);
        // What it took and never answered waits for the next agent again, instead of showing "on it" forever.
        for (thread_id, review_id) in self.state.get_mut().working.drain() {
            let working = services
                .store
                .get(&review_id)
                .and_then(|review| {
                    review
                        .threads
                        .into_iter()
                        .find(|thread| thread.id == thread_id)
                })
                .is_some_and(|thread| thread.agent_state == Some(AgentState::Working));
            if working {
                services
                    .store
                    .set_agent_state(&review_id, &thread_id, AgentState::Queued)
                    .map_err(|error| {
                        log::error!("[co-review] giving back {thread_id} failed: {error}")
                    })
                    .ok();
            }
        }
    }
}

/// The co-reviewer: the tools every session calls, over the shared services.
pub struct CoReviewer {
    pub services: Services,
    /// The repository the app was started for: the default `root`.
    default_root: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OpenReviewArguments {
    pub review_id: Option<String>,
    pub root: Option<String>,
    pub dir: Option<String>,
    pub markdown: Option<String>,
    pub patch: Option<String>,
    pub diff: Option<String>,
    pub patch_name: Option<String>,
    pub store_path: Option<String>,
    pub title: Option<String>,
    pub open: Option<bool>,
    pub openspec: Option<String>,
    pub overview: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AwaitArguments {
    pub review_id: Option<String>,
    pub timeout_sec: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyArguments {
    pub thread_id: String,
    pub body: String,
    pub resolve: Option<bool>,
    pub review_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddFindingsArguments {
    pub review_id: Option<String>,
    pub findings: Vec<Finding>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub path: Option<String>,
    pub line: Option<u32>,
    pub end_line: Option<u32>,
    pub body: String,
    pub severity: Option<Severity>,
    pub id: Option<String>,
    pub target: Option<String>,
    pub anchor: Option<Map<String, Value>>,
    pub labels: Option<Vec<String>>,
    pub verdict: Option<String>,
    pub status: Option<ThreadStatus>,
    pub proposal: Option<FindingProposal>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingProposal {
    pub before: String,
    pub after: String,
    pub path: Option<String>,
    pub start_line: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskArguments {
    pub review_id: Option<String>,
    pub question: String,
    pub options: Vec<String>,
    pub thread_id: Option<String>,
    pub timeout_sec: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PostArguments {
    pub review_id: Option<String>,
    pub summary: Option<String>,
    pub comments: Option<Vec<RewordedComment>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewordedComment {
    pub thread_id: String,
    pub body: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RepoMapArguments {
    pub root: Option<String>,
    pub path: Option<String>,
    pub query: Option<String>,
    pub refresh: Option<bool>,
    pub overview: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GetReviewArguments {
    pub review_id: Option<String>,
    pub root: Option<String>,
}

/// How long an unanswered `ask_reviewer` question is kept; then its buttons show it was cancelled.
const QUESTION_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
/// How long `post_review_to_github` waits for the reviewer to confirm.
const POST_CONFIRMATION: Duration = Duration::from_secs(15 * 60);

macro_rules! fail_on {
    ($result:expr) => {
        match $result {
            Ok(value) => value,
            Err(error) => return ToolOutput::fail(error.to_string()),
        }
    };
}

impl CoReviewer {
    pub fn new(services: Services, default_root: Option<&Path>) -> Self {
        Self {
            services,
            default_root: default_root.map(|root| real_path(&root.to_string_lossy())),
        }
    }

    /// The reviewer closed the last window of `workspace_root` (a file URI) or quit: its MCP reviews are closed, and
    /// every agent waiting on them hears it once, also after Co-Review restarts.
    pub fn window_closed(&self, workspace_root: &str) {
        self.mark_closed(workspace_root, true);
    }

    /// A window of `workspace_root` opened again: its reviews are not closed any more.
    pub fn window_opened(&self, workspace_root: &str) {
        self.mark_closed(workspace_root, false);
    }

    fn mark_closed(&self, workspace_root: &str, closed: bool) {
        let store = &self.services.store;
        let reviews = store
            .list_for_window(workspace_root)
            .into_iter()
            .filter(|review| {
                review
                    .agent
                    .as_ref()
                    .is_some_and(|agent| agent.transport == Some(AgentTransport::Mcp))
            });
        for review in reviews {
            let result = if closed {
                store
                    .set_closed(&review.id, Some(ReviewClosed { at: now_millis() }))
                    .map(|_| ())
            } else if review.closed.is_some() {
                store.set_closed(&review.id, None).map(|_| ())
            } else {
                Ok(())
            };
            if let Err(error) = result {
                log::error!("[co-review] recording a closed window failed: {error}");
            }
        }
    }

    // ------------------------------------------------------------------ open_review

    pub async fn open_review(
        self: &Arc<Self>,
        session: &Arc<Session>,
        arguments: OpenReviewArguments,
    ) -> ToolOutput {
        let agent = session.agent();
        let store = &self.services.store;
        let mut patch = arguments.patch.clone();
        // A diff Co-Review ran: the repository and the diff give the review its code folder.
        let mut change: Option<(String, String)> = None;
        if let Some(range) = arguments
            .diff
            .as_deref()
            .filter(|_| arguments.review_id.is_none())
        {
            let Some(repo) = arguments
                .root
                .as_deref()
                .map(real_path)
                .or_else(|| session.default_root())
            else {
                return ToolOutput::fail("`diff` needs the repository: pass `root`.");
            };
            let taken = {
                let (repo, range) = (repo.clone(), range.to_string());
                let (dir, name) = (arguments.dir.clone(), arguments.patch_name.clone());
                tokio::task::spawn_blocking(move || {
                    review_bundle::diff::take_diff(
                        Path::new(&repo),
                        &range,
                        dir.as_deref().map(Path::new),
                        name.as_deref(),
                    )
                })
                .await
            };
            let (text, taken) = fail_on!(fail_on!(taken));
            if let review_bundle::diff::TakenDiff::Inline(inline) = taken {
                patch = Some(inline);
            }
            change = Some((repo, text));
        }

        let existing = arguments.review_id.as_deref().and_then(|id| store.get(id));
        if let Some(id) = &arguments.review_id
            && existing.is_none()
        {
            return ToolOutput::fail(format!("No review {id}."));
        }
        let mut extra = Map::new();
        let (mut review, workspace) = if let Some(existing) = existing {
            let workspace = window_root_of(&existing);
            if let Some(bundle) = &existing.bundle {
                let document = document_of(&bundle.dir);
                let patches = review_bundle::service::patches_of(Path::new(&bundle.dir));
                extra = self.directory_extra(document.as_deref(), patches.len(), &bundle.dir);
            }
            (existing, workspace)
        } else if arguments.dir.is_some() || arguments.markdown.is_some() || patch.is_some() {
            let bundles = Arc::clone(&self.services.bundles);
            let request = review_bundle::OpenBundleParams {
                dir: arguments.dir.clone().map(PathBuf::from),
                markdown: arguments.markdown.clone(),
                patch: patch.clone(),
                patch_name: arguments.patch_name.clone(),
                title: arguments.title.clone(),
                store_path: arguments.store_path.clone().map(PathBuf::from),
                openspec: arguments.openspec.clone().map(PathBuf::from),
                repo: change.as_ref().map(|(repo, _)| PathBuf::from(repo)),
                author: agent.clone(),
            };
            let opened = tokio::task::spawn_blocking(move || bundles.open(request)).await;
            let opened = fail_on!(fail_on!(opened));
            let dir = opened.dir.to_string_lossy().into_owned();
            let document = opened
                .doc_file
                .map(|file| file.to_string_lossy().into_owned());
            let mut review = opened.review;
            let mut workspace = dir.clone();
            if let (Some((repo, text)), Some(range)) = (&change, &arguments.diff) {
                // The window opens on the code; the diff page stays the front page. Without it, the review directory.
                match self.prepare_code(&review, repo, range, text).await {
                    Ok(folder) => {
                        workspace = folder.to_string_lossy().into_owned();
                        review = store.get(&review.id).unwrap_or(review);
                    }
                    Err(error) => log::error!(
                        "[co-review] preparing the code of the review failed: {error:#}"
                    ),
                }
            }
            extra = self.directory_extra(document.as_deref(), opened.patches.len(), &dir);
            (review, workspace)
        } else {
            let Some(workspace) = arguments
                .root
                .as_deref()
                .map(real_path)
                .or_else(|| session.default_root())
            else {
                return ToolOutput::fail(
                    "No repository: pass `root` (absolute path), or `dir` / `markdown` / `patch`.",
                );
            };
            let workspace_root = path_to_file_uri(&workspace);
            // Called again without a title (e.g. to show the review once findings are in): the same review.
            let current = session.current_review_id().and_then(|id| store.get(&id));
            let found = match current {
                Some(current)
                    if arguments.title.is_none() && current.workspace_root == workspace_root =>
                {
                    Some(current)
                }
                _ => store.list(&workspace_root).into_iter().next(),
            };
            let review = match found {
                Some(review) if arguments.title.is_none() => review,
                _ => fail_on!(
                    store.create(CreateReviewParams {
                        workspace_root,
                        title: arguments
                            .title
                            .clone()
                            .unwrap_or_else(|| format!("Review with {}", agent.name)),
                        scope: ReviewScope::Repository,
                        author: agent.clone(),
                    })
                ),
            };
            (review, workspace)
        };

        review = fail_on!(store.set_agent(
            &review.id,
            Some(AgentConfig {
                id: agent.id.clone(),
                name: agent.name.clone(),
                transport: Some(AgentTransport::Mcp),
                command: None,
                args: None,
                settings: None,
                settings_label: None,
                extra: Map::new(),
            }),
        ));
        if let Some(overview) = arguments
            .overview
            .as_deref()
            .map(str::trim)
            .filter(|overview| !overview.is_empty())
        {
            review = fail_on!(store.set_overview(&review.id, overview.to_string()));
        }
        {
            let mut state = session.state.lock();
            state.current_review_id = Some(review.id.clone());
            // A round no agent has picked up yet (submitted while none was connected) is still delivered.
            state.consumed_submit = review.verdict.as_ref().map_or(0, |verdict| {
                verdict
                    .count
                    .saturating_sub(u32::from(verdict.to_agent == Some(VerdictToAgent::Waiting)))
            });
        }
        self.start_channel(session, &review);
        self.services.presence.seen(&review.id);

        let shown = arguments.open != Some(false);
        let mut note = format!(
            "Prepared, not shown to the reviewer yet. Add your first-pass findings, then call open_review({{ reviewId: \"{}\" }}) to show it.",
            review.id
        );
        if shown {
            if review.closed.is_some() {
                review = fail_on!(store.set_closed(&review.id, None));
            }
            let (pages, review_id) = (Arc::clone(&self.services.pages), review.id.clone());
            let written = tokio::task::spawn_blocking(move || pages.write(&review_id, true)).await;
            if let Ok(Err(error)) = written {
                log::warn!("[co-review] writing the overview page failed: {error:#}");
            }
            note = self
                .services
                .windows
                .show(&review.id, Path::new(&workspace), &session.id);
        }
        ToolOutput::json(
            &Object::new()
                .set("reviewId", &review.id)
                .set("title", &review.title)
                .set("openThreads", count(&review, ThreadStatus::Open))
                .set("proposedFindings", count(&review, ThreadStatus::Proposed))
                .spread(extra)
                .set("note", note)
                .set("hint", "Tell the reviewer the review is open in Co-Review, then loop await_reviewer → reply until it returns their Submit.")
                .build(),
        )
    }

    fn directory_extra(
        &self,
        document: Option<&str>,
        patches: usize,
        dir: &str,
    ) -> Map<String, Value> {
        Object::new()
            .set("hasDoc", document.is_some())
            .set("patches", patches)
            .set("dir", dir)
            .spread(format_of(document))
            .into_map()
    }

    async fn prepare_code(
        &self,
        review: &Review,
        repo: &str,
        range: &str,
        text: &str,
    ) -> anyhow::Result<PathBuf> {
        let files = review_bundle::patch::parse_patch(text);
        let changed = review_bundle::patch::changed_lines(&files);
        // Its blast radius from the repository's Graphify graph, when it has one: those files come first too.
        let related =
            std::fs::read_to_string(Path::new(repo).join("graphify-out").join("graph.json"))
                .ok()
                .and_then(|graph| serde_json::from_str::<review_graph::GraphifyGraph>(&graph).ok())
                .and_then(|graph| review_graph::blast_radius(&graph, &changed))
                .map(|radius| radius.files)
                .unwrap_or_default();
        let paths: Vec<String> = files.into_iter().map(|file| file.path).collect();
        let (folders, review, repo, range) = (
            Arc::clone(&self.services.code_folders),
            review.clone(),
            repo.to_string(),
            range.to_string(),
        );
        tokio::task::spawn_blocking(move || {
            folders.prepare(&review, Path::new(&repo), &range, &paths, &related)
        })
        .await?
    }

    // ------------------------------------------------------------------ await_reviewer

    pub async fn await_reviewer(
        &self,
        session: &Arc<Session>,
        arguments: AwaitArguments,
        cancel: CancellationToken,
    ) -> ToolOutput {
        let timeout = Duration::from_secs(arguments.timeout_sec.unwrap_or(240));
        let review = session.resolve_review(arguments.review_id.as_deref(), None);
        self.wait_for_reviewer(session, review, timeout, cancel)
            .await
            .0
    }

    /// Waits for the reviewer's next move in `initial`: threads waiting for this agent, or a Submit not yet returned
    /// to this session (comments first: the submitted batch then has the answers in it). Every call that waits goes
    /// through here, so none of them can miss what the others would return. Also returns the result as JSON.
    async fn wait_for_reviewer(
        &self,
        session: &Arc<Session>,
        initial: Option<Review>,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> (ToolOutput, Option<Value>) {
        let Some(initial) = initial else {
            return (ToolOutput::fail("No review. Call open_review first."), None);
        };
        let agent = session.agent();
        let store = &self.services.store;
        let _listening = self.services.presence.listen(&initial.id);
        let changes = store.subscribe_changes();
        let deadline = Instant::now() + timeout;
        loop {
            while changes.try_recv().is_ok() {}
            if let Err(error) = self.services.bundles.resolve_anchors(&initial.id) {
                log::warn!("[co-review] re-anchoring {} failed: {error:#}", initial.id);
            }
            let Some(review) = store.get(&initial.id) else {
                return (
                    ToolOutput::fail(format!("Review {} was deleted.", initial.id)),
                    None,
                );
            };
            let waiting = self.waiting_threads(&review, &agent, session);
            let submitted = review
                .verdict
                .as_ref()
                .filter(|verdict| verdict.count > session.state.lock().consumed_submit);
            if let Some(verdict) = submitted
                && waiting.is_empty()
            {
                session.state.lock().consumed_submit = verdict.count;
                if verdict.to_agent == Some(VerdictToAgent::Waiting)
                    && let Err(error) = store.verdict_delivered(&review.id)
                {
                    log::error!("[co-review] recording the delivered verdict failed: {error}");
                }
                // Approved: this agent is done with the review, and a window it showed can close.
                if verdict.decision == ReviewDecision::Approve {
                    self.services.windows.done(
                        &session.id,
                        &format!("You approved the review, and {} has it.", agent.name),
                        Some(&path_to_file_uri(&window_root_of(&review))),
                    );
                }
                let value = Object::new()
                    .spread(self.batch(&review))
                    .set(
                        "hint",
                        "The reviewer submitted: act on the decision and the open comments.",
                    )
                    .build();
                return (ToolOutput::json(&value), Some(value));
            }
            if !waiting.is_empty() {
                {
                    // All marked delivered before any write: each write wakes the channel, which must not push them too.
                    let mut state = session.state.lock();
                    for thread in &waiting {
                        if let Some(last) = thread.messages.last() {
                            state.delivered.insert(last.id.clone());
                        }
                    }
                }
                for thread in &waiting {
                    // A choice is the answer to the agent's own question: handed over, nothing to reply to.
                    let choice = thread
                        .messages
                        .last()
                        .is_some_and(|message| message.choice.is_some());
                    if !choice {
                        session
                            .state
                            .lock()
                            .working
                            .insert(thread.id.clone(), review.id.clone());
                    }
                    let state = if choice {
                        AgentState::Idle
                    } else {
                        AgentState::Working
                    };
                    if let Err(error) = store.set_agent_state(&review.id, &thread.id, state) {
                        log::error!(
                            "[co-review] marking {} as handed over failed: {error}",
                            thread.id
                        );
                    }
                }
                let value = self.comment_batch(&review, &waiting);
                return (ToolOutput::json(&value), Some(value));
            }
            // Told once per session: every agent on the review hears it, also a new session after a restart.
            if let Some(closed) = &review.closed {
                let mut state = session.state.lock();
                if state.told_closed != Some(closed.at) {
                    state.told_closed = Some(closed.at);
                    let value = closed_notice(&review);
                    return (ToolOutput::json(&value), Some(value));
                }
            }
            let now = Instant::now();
            if now >= deadline || cancel.is_cancelled() {
                let value = json!({ "status": "pending", "hint": "Nothing from the reviewer yet. Call it again to keep waiting." });
                return (ToolOutput::json(&value), Some(value));
            }
            let review_id = initial.id.clone();
            let next_change = async {
                while let Ok(change) = changes.recv().await {
                    if let ReviewChange::Changed { review } = change
                        && review.id == review_id
                    {
                        return;
                    }
                }
                // The store went away: nothing will change any more.
                std::future::pending::<()>().await;
            };
            tokio::select! {
                _ = next_change => {}
                _ = tokio::time::sleep(deadline - now) => {}
                _ = cancel.cancelled() => {}
            }
        }
    }

    /// The threads to hand this agent: the reviewer's latest message is for it, not yet handed to this session, and
    /// not already taken (`idle`: e.g. a choice ask_reviewer returned). A thread another session took (`working`) is
    /// handed again: that session may be gone, and a repeat beats a question nobody answers.
    fn waiting_threads(
        &self,
        review: &Review,
        agent: &Participant,
        session: &Session,
    ) -> Vec<ReviewThread> {
        let state = session.state.lock();
        review
            .threads
            .iter()
            .filter(|thread| {
                waits_for_agent(review, thread, &agent.id)
                    && thread.agent_state != Some(AgentState::Idle)
                    && thread
                        .messages
                        .last()
                        .is_some_and(|last| !state.delivered.contains(&last.id))
            })
            .cloned()
            .collect()
    }

    /// The submitted batch (await_reviewer) and get_review's snapshot.
    fn batch(&self, review: &Review) -> Map<String, Value> {
        let open: Vec<&ReviewThread> = review
            .threads
            .iter()
            .filter(|thread| thread.status == ThreadStatus::Open)
            .collect();
        let document = review
            .bundle
            .as_ref()
            .and_then(|bundle| document_of(&bundle.dir));
        let verdict = review.verdict.as_ref();
        let root = root_of(review);
        let code_root = code_root_of(review);
        Object::new()
            .set(
                "status",
                if verdict.is_some() {
                    "submitted"
                } else {
                    "no-submit"
                },
            )
            .set("reviewId", &review.id)
            .maybe("decision", verdict.map(|verdict| verdict.decision))
            .maybe("summary", verdict.map(|verdict| &verdict.summary))
            .maybe(
                "submittedAt",
                verdict.map(|verdict| iso_time(verdict.submitted_at)),
            )
            .set("round", verdict.map_or(0, |verdict| verdict.count))
            .set(
                "doc",
                Object::new()
                    .set(
                        "slug",
                        document.as_deref().map(basename).unwrap_or_default(),
                    )
                    .set(
                        "version",
                        review
                            .bundle
                            .as_ref()
                            .and_then(|bundle| bundle.doc_version)
                            .unwrap_or(1),
                    )
                    .spread(format_of(document.as_deref()))
                    .build(),
            )
            .set(
                "comments",
                open.iter()
                    .map(|thread| comment_of(thread, &root, Map::new(), &code_root))
                    .collect::<Vec<_>>(),
            )
            .set("acceptedSuggestions", accepted_suggestions(review))
            .set("openCount", open.len())
            .set("proposedCount", count(review, ThreadStatus::Proposed))
            .into_map()
    }

    /// The await_reviewer comment batch, plus the threads to answer with full context.
    fn comment_batch(&self, review: &Review, waiting: &[ReviewThread]) -> Value {
        // Proposed findings are part of the conversation too (the reviewer asks before accepting or dismissing).
        let unresolved: Vec<&ReviewThread> = review
            .threads
            .iter()
            .filter(|thread| thread.status != ThreadStatus::Resolved)
            .collect();
        let last_role = |thread: &ReviewThread| {
            thread
                .messages
                .last()
                .map(|message| message.author.kind.clone())
        };
        let needs_reply = |thread: &ReviewThread| last_role(thread) == Some(ParticipantKind::Human);
        let root = root_of(review);
        let code_root = code_root_of(review);
        Object::new()
            .set("status", "comment")
            .set("reviewId", &review.id)
            .set(
                "comments",
                unresolved
                    .iter()
                    .map(|thread| {
                        let extra = Object::new()
                            .maybe("lastRole", last_role(thread))
                            .set("needsReply", needs_reply(thread))
                            .into_map();
                        comment_of(thread, &root, extra, &code_root)
                    })
                    .collect::<Vec<_>>(),
            )
            .set("openCount", unresolved.iter().filter(|thread| thread.status == ThreadStatus::Open).count())
            .set("needsReply", unresolved.iter().filter(|thread| needs_reply(thread)).count())
            .set("threads", waiting.iter().map(|thread| describe(review, thread)).collect::<Vec<_>>())
            .set(
                "hint",
                "reply(threadId, body) to answer (edit the reviewed files on disk if needed — the view live-reloads), then wait again with the same call.",
            )
            .set("howToAnswer", ANSWER_STYLE_SHORT)
            .build()
    }

    // ------------------------------------------------------------------ reply

    pub fn reply(&self, session: &Arc<Session>, arguments: ReplyArguments) -> ToolOutput {
        let review = session.resolve_review(arguments.review_id.as_deref(), None);
        let found = review.as_ref().and_then(|review| {
            find_thread(review, arguments.thread_id.trim()).map(|thread| (review, thread))
        });
        let Some((review, thread)) = found else {
            return ToolOutput::fail(format!("No thread \"{}\".", arguments.thread_id));
        };
        let agent = session.agent();
        let store = &self.services.store;
        let updated = fail_on!(store.add_message(
            &review.id,
            &thread.id,
            arguments.body,
            agent.clone(),
            None
        ));
        fail_on!(store.set_agent_state(&review.id, &thread.id, AgentState::Idle));
        session.state.lock().working.remove(&thread.id);
        if arguments.resolve == Some(true) {
            fail_on!(store.set_thread_status(
                &review.id,
                &thread.id,
                ThreadStatus::Resolved,
                agent
            ));
        }
        self.services.presence.seen(&review.id);
        ToolOutput::json(&json!({
            "ok": true,
            "thread": thread_ref(&updated),
            "threadId": updated.id,
            "messages": updated.messages.len(),
        }))
    }

    // ------------------------------------------------------------------ add_findings

    pub fn add_findings(
        &self,
        session: &Arc<Session>,
        arguments: AddFindingsArguments,
    ) -> ToolOutput {
        let Some(review) = session.resolve_review(arguments.review_id.as_deref(), None) else {
            return ToolOutput::fail("No review. Call open_review first.");
        };
        let agent = session.agent();
        let store = &self.services.store;
        let dir = root_of(&review);
        let mut ids = Vec::new();
        for finding in arguments.findings {
            if let (Some(_), Some(target)) = (&finding.anchor, &finding.target)
                && let Some(page) = target.strip_prefix("doc:")
                && !is_markdown(page)
            {
                return ToolOutput::fail(format!(
                    "{target}: only Markdown pages take a doc: target; comment on other files with path and line."
                ));
            }
            if let Some(anchor) = &finding.anchor {
                let document = document_of(&dir);
                let seeded = review_bundle::findings::Finding {
                    id: finding
                        .id
                        .clone()
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                    proposal: finding.proposal.as_ref().map(|proposal| {
                        review_bundle::findings::Proposal {
                            before: proposal.before.clone(),
                            after: proposal.after.clone(),
                            path: proposal.path.clone(),
                            start_line: proposal.start_line,
                        }
                    }),
                    target: finding.target.clone(),
                    anchor: Value::Object(anchor.clone()),
                    severity: finding.severity.map(|severity| name_of(&severity)),
                    labels: finding.labels.clone(),
                    body: finding.body.clone(),
                    verdict: finding.verdict.clone(),
                };
                fail_on!(self.services.bundles.add_finding(
                    &review.id,
                    Path::new(&dir),
                    document.as_deref().map(Path::new),
                    &seeded,
                    Some(agent.clone()),
                ));
            } else if let Some(path) = &finding.path {
                let root = code_root_of(&review);
                let file = resolve(&root, path);
                if !is_within(&root, &file) {
                    return ToolOutput::fail(format!("{path} is outside the repository ({root})."));
                }
                let location = match finding.line {
                    Some(line) => self.location_for(&file, line, finding.end_line),
                    None => path_location(&root, &file),
                };
                let options = ThreadOptions {
                    intent: Some(ThreadIntent::Comment),
                    severity: finding.severity,
                    status: finding.status,
                    origin: Some("finding".to_string()),
                    source_id: None,
                    labels: finding.labels.clone(),
                    proposal: None,
                };
                fail_on!(store.create_thread(
                    &review.id,
                    location,
                    finding.body.clone(),
                    agent.clone(),
                    options
                ));
            } else {
                return ToolOutput::fail(
                    "Each finding needs either `anchor` (with `target`) or `path`.",
                );
            }
            let latest = store
                .get(&review.id)
                .map_or(0, |review| review.next_thread_number);
            ids.push(format!("t{}", latest.saturating_sub(1)));
        }
        ToolOutput::json(&json!({
            "created": ids.len(),
            "ids": ids,
            "hint": "Findings with an anchor are proposed: only the ones the reviewer accepts come back when they submit.",
        }))
    }

    /// A semantic location (range, anchor text, Tree-sitter tokens, symbol) from a file and lines.
    fn location_for(&self, file: &str, line: u32, end_line: Option<u32>) -> CodeLocation {
        let uri = path_to_file_uri(file);
        let Ok(text) = std::fs::read_to_string(file) else {
            return CodeLocation {
                uri: Some(uri),
                ..location(LocationKind::File)
            };
        };
        let lines: Vec<&str> = text.split('\n').collect();
        let count = lines.len() as u32;
        let start = line.max(1).min(count).saturating_sub(1);
        let end = end_line
            .unwrap_or(line)
            .max(line)
            .min(count)
            .saturating_sub(1);
        let line_at = |index: u32| lines.get(index as usize).copied();
        let end_character = line_at(end).map_or(0, |text| text.encode_utf16().count() as u32);
        let range = Range {
            start: Position {
                line: start,
                character: 0,
            },
            end: Position {
                line: end,
                character: end_character,
            },
        };
        let language = Path::new(file).extension().and_then(|extension| {
            review_index::language_for_extension(&format!(".{}", extension.to_string_lossy()))
        });
        let syntax = language
            .and_then(|language| self.services.syntax.capture_anchor(language, &text, &range));
        let symbol = language.and_then(|language| {
            self.services
                .syntax
                .enclosing_symbol(language, &text, start)
        });
        let selected: Vec<&str> = (start..=end).filter_map(line_at).collect();
        CodeLocation {
            kind: if start == end {
                LocationKind::Line
            } else {
                LocationKind::Range
            },
            uri: Some(uri),
            range: Some(range),
            symbol,
            anchor: Some(CodeAnchor {
                text: selected.join("\n"),
                before: start.checked_sub(1).and_then(line_at).map(str::to_string),
                after: line_at(end + 1).map(str::to_string),
                syntax,
            }),
            ..location(LocationKind::Line)
        }
    }

    // ------------------------------------------------------------------ ask_reviewer

    pub async fn ask_reviewer(
        self: &Arc<Self>,
        session: &Arc<Session>,
        arguments: AskArguments,
        cancel: CancellationToken,
    ) -> ToolOutput {
        let review = session.resolve_review(arguments.review_id.as_deref(), None);
        let in_thread = match (&review, &arguments.thread_id) {
            (Some(review), Some(id)) => find_thread(review, id.trim()).cloned(),
            _ => None,
        };
        let Some(review) = review else {
            return ToolOutput::fail("No review. Call open_review first.");
        };
        if let Some(id) = &arguments.thread_id
            && in_thread.is_none()
        {
            return ToolOutput::fail(format!("No thread \"{id}\"."));
        }
        let agent = session.agent();
        let store = &self.services.store;
        let asked_in_thread = in_thread.is_some();
        let thread = match in_thread {
            Some(thread) => thread,
            None => {
                fail_on!(store.create_thread(
                    &review.id,
                    location(LocationKind::Repository),
                    arguments.question.clone(),
                    agent.clone(),
                    ThreadOptions::default(),
                ))
                .1
            }
        };
        let request = PermissionRequest {
            id: uuid::Uuid::new_v4().to_string(),
            title: arguments.question.clone(),
            options: arguments
                .options
                .iter()
                .enumerate()
                .map(|(index, name)| PermissionOption {
                    id: index.to_string(),
                    name: name.clone(),
                    kind: "allow_once".to_string(),
                })
                .collect(),
            outcome: None,
        };
        let body = if asked_in_thread {
            arguments.question.clone()
        } else {
            String::new()
        };
        let message_id = fail_on!(start_permission_message(
            self,
            &review.id,
            &thread.id,
            agent,
            body,
            request.clone()
        ));
        fail_on!(store.set_agent_state(&review.id, &thread.id, AgentState::WaitingForHuman));

        // The answer can come at any time, also after this call returned: it lands in the thread as the reviewer's
        // message, which await_reviewer delivers like any other.
        let choice: Arc<Mutex<Option<String>>> = Arc::default();
        let recorded = {
            let reviewer = Arc::clone(self);
            let choice = Arc::clone(&choice);
            let (review, thread_id, options) =
                (review.clone(), thread.id.clone(), arguments.options.clone());
            tokio::spawn(async move {
                let option = reviewer
                    .services
                    .decisions
                    .wait(&request.id, QUESTION_LIFETIME)
                    .await;
                let chosen = option
                    .as_deref()
                    .and_then(|option| option.parse::<usize>().ok())
                    .and_then(|index| options.get(index).cloned());
                *choice.lock() = chosen.clone();
                let result = reviewer.record_choice(
                    &review,
                    &thread_id,
                    &message_id,
                    request,
                    option,
                    chosen,
                );
                result
                    .map_err(|error| {
                        log::error!("[co-review] recording the reviewer's choice failed: {error:#}")
                    })
                    .ok()
                    .flatten()
            })
        };
        let timeout = Duration::from_secs(arguments.timeout_sec.unwrap_or(900));
        let (output, payload) = self
            .wait_for_reviewer(session, Some(review.clone()), timeout, cancel)
            .await;
        let Some(payload) = payload.filter(|_| !output.is_error) else {
            return output;
        };
        let chosen = choice.lock().clone();
        if chosen.is_some() {
            // This call returns the choice: it is delivered, not something to wait for again.
            if let Ok(Some(message_id)) = recorded.await {
                session.state.lock().delivered.insert(message_id);
                if let Err(error) = store.set_agent_state(&review.id, &thread.id, AgentState::Idle)
                {
                    log::error!("[co-review] {error}");
                }
            }
        }
        let question = Object::new()
            .set("threadId", thread_ref(&thread))
            .set(
                "status",
                if chosen.is_some() {
                    "answered"
                } else {
                    "pending"
                },
            )
            .maybe("choice", chosen)
            .build();
        let payload = match payload {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        ToolOutput::json(
            &Object::new()
                .spread(payload)
                .set("question", question)
                .build(),
        )
    }

    /// Writes the reviewer's answer to a question into its thread; returns the id of their message.
    fn record_choice(
        &self,
        review: &Review,
        thread_id: &str,
        message_id: &str,
        mut request: PermissionRequest,
        option: Option<String>,
        chosen: Option<String>,
    ) -> anyhow::Result<Option<String>> {
        let store = &self.services.store;
        request.outcome = Some(option.unwrap_or_else(|| "cancelled".to_string()));
        update_permission(self, &review.id, thread_id, message_id, request)?;
        let Some(chosen) = chosen else {
            store.set_agent_state(&review.id, thread_id, AgentState::Idle)?;
            return Ok(None);
        };
        let reviewer = review_store::current_user(Path::new(&uri_to_path(&review.workspace_root)));
        let answered = store.add_message(
            &review.id,
            thread_id,
            format!("Chose: **{chosen}**"),
            reviewer,
            Some(chosen),
        )?;
        Ok(answered.messages.last().map(|message| message.id.clone()))
    }

    // ------------------------------------------------------------------ post_review_to_github

    pub async fn post_review_to_github(
        &self,
        session: &Arc<Session>,
        arguments: PostArguments,
    ) -> ToolOutput {
        let review = session.resolve_review(arguments.review_id.as_deref(), None);
        let target = review.as_ref().and_then(|review| {
            let dir = review.bundle.as_ref()?.dir.clone();
            review_github::resolve_target(Path::new(&dir), review)
        });
        let (Some(review), Some(target)) = (review, target) else {
            return ToolOutput::fail(
                "This review is not a GitHub pull request: its review directory needs a PR.md with `repo: <owner>/<name>` and `pr: <number>`.",
            );
        };
        let mut reworded = HashMap::new();
        for comment in arguments.comments.unwrap_or_default() {
            let Some(thread) = find_thread(&review, comment.thread_id.trim()) else {
                return ToolOutput::fail(format!("No thread \"{}\".", comment.thread_id));
            };
            if !comment.body.trim().is_empty() {
                reworded.insert(thread.id.clone(), comment.body.trim().to_string());
            }
        }
        let decision = review
            .verdict
            .as_ref()
            .map_or(ReviewDecision::Comment, |verdict| verdict.decision);
        let summary = arguments
            .summary
            .clone()
            .or_else(|| {
                review
                    .verdict
                    .as_ref()
                    .map(|verdict| verdict.summary.clone())
            })
            .unwrap_or_default();
        let composed = review_github::compose(&review, decision, &summary, &reworded);
        let preview = review_github::preview(&review, decision, &summary, &reworded);
        let round = review.verdict.as_ref().map_or(0, |verdict| verdict.count);
        let again = match target.posted_round {
            Some(posted) if posted == round => format!(
                " This round was already posted ({}).",
                target.posted_url.as_deref().unwrap_or("undefined")
            ),
            _ => String::new(),
        };
        let lines = preview.comments.len();
        let question = format!(
            "Post this review to GitHub, {}#{}? As \"{}\", with {lines} line comment{}{}.{again}",
            target.repo,
            target.number,
            name_of(&decision),
            if lines == 1 { "" } else { "s" },
            if composed.elsewhere > 0 {
                format!(" and {} in the review's body", composed.elsewhere)
            } else {
                String::new()
            },
        );
        let agent = session.agent();
        let store = &self.services.store;
        let (_, thread) = fail_on!(store.create_thread(
            &review.id,
            location(LocationKind::Repository),
            question.clone(),
            agent.clone(),
            ThreadOptions::default(),
        ));
        let request = PermissionRequest {
            id: uuid::Uuid::new_v4().to_string(),
            title: question,
            options: vec![
                PermissionOption {
                    id: "post".to_string(),
                    name: "Post to GitHub".to_string(),
                    kind: "allow_once".to_string(),
                },
                PermissionOption {
                    id: "skip".to_string(),
                    name: "Don't post".to_string(),
                    kind: "reject_once".to_string(),
                },
            ],
            outcome: None,
        };
        // Exactly what goes out, so the reviewer confirms the words, not just the counts.
        let body = format!(
            "This is what will be posted:\n\n{}",
            review_github::preview_markdown(&preview)
        );
        let message_id = fail_on!(start_permission_message(
            self,
            &review.id,
            &thread.id,
            agent.clone(),
            body,
            request.clone()
        ));
        fail_on!(store.set_agent_state(&review.id, &thread.id, AgentState::WaitingForHuman));
        let choice = self
            .services
            .decisions
            .wait(&request.id, POST_CONFIRMATION)
            .await;
        let mut answered = request;
        answered.outcome = Some(choice.clone().unwrap_or_else(|| "cancelled".to_string()));
        fail_on!(update_permission(
            self,
            &review.id,
            &thread.id,
            &message_id,
            answered
        ));
        fail_on!(store.set_agent_state(&review.id, &thread.id, AgentState::Idle));
        if choice.as_deref() != Some("post") {
            store
                .set_thread_status(&review.id, &thread.id, ThreadStatus::Resolved, agent)
                .ok();
            return ToolOutput::json(&json!({
                "status": if choice.is_none() { "pending" } else { "declined" },
                "hint": "Nothing was posted. Don't post unless the reviewer asks again.",
            }));
        }
        let runner = Arc::clone(&self.services.github);
        let (posting, posted_target, posted_reworded, posted_summary) = (
            review.clone(),
            target.clone(),
            reworded.clone(),
            summary.clone(),
        );
        let posted = tokio::task::spawn_blocking(move || {
            review_github::publish(
                runner.as_ref(),
                &posting,
                &posted_target,
                decision,
                &posted_summary,
                &posted_reworded,
                None,
            )
        })
        .await
        .map_err(anyhow::Error::from)
        .and_then(|result| result);
        match posted {
            Ok(posted) => {
                if let Some(bundle) = store.get(&review.id).and_then(|review| review.bundle) {
                    let bundle = ReviewBundle {
                        github: Some(review_model::BundleGitHub {
                            url: posted.url.clone(),
                            round,
                            posted_at: now_millis(),
                        }),
                        ..bundle
                    };
                    store.set_bundle(&review.id, Some(bundle)).ok();
                }
                store
                    .add_message(
                        &review.id,
                        &thread.id,
                        format!("Posted to GitHub: {}", posted.url),
                        agent.clone(),
                        None,
                    )
                    .ok();
                store
                    .set_thread_status(&review.id, &thread.id, ThreadStatus::Resolved, agent)
                    .ok();
                ToolOutput::json(&json!({
                    "status": "posted",
                    "url": posted.url,
                    "comments": posted.comments,
                    "inBody": posted.in_body,
                    "event": posted.event,
                    "notes": posted.notes,
                }))
            }
            Err(error) => {
                store
                    .add_message(
                        &review.id,
                        &thread.id,
                        format!("Posting to GitHub failed: {error}"),
                        agent,
                        None,
                    )
                    .ok();
                ToolOutput::fail(error.to_string())
            }
        }
    }

    // ------------------------------------------------------------------ repo_map

    pub async fn repo_map(
        &self,
        session: &Arc<Session>,
        arguments: RepoMapArguments,
    ) -> ToolOutput {
        let root = arguments.root.as_deref().map(real_path);
        let review = session.resolve_review(None, root.as_deref());
        let workspace = root.or_else(|| match &review {
            Some(review)
                if review
                    .bundle
                    .as_ref()
                    .is_none_or(|bundle| bundle.code.is_some()) =>
            {
                Some(code_root_of(review))
            }
            _ => session.default_root(),
        });
        let Some(workspace) = workspace else {
            return ToolOutput::fail("No repository: pass `root` (absolute path).");
        };
        let viewed = review
            .filter(|review| review.bundle.is_none())
            .and_then(|review| review.viewed);
        let output = tokio::task::spawn_blocking(move || {
            let root = Path::new(&workspace);
            // Every call re-checks the files (the map is cached by content), so `refresh` needs nothing more.
            if arguments.overview == Some(true) {
                return RepositorySections.repository_overview(root, &viewed.unwrap_or_default());
            }
            let index = review_index::RepoIndex::new();
            let map = index.build(root)?;
            let options = review_index::RenderOptions {
                path: arguments.path,
                query: arguments.query,
                ..Default::default()
            };
            index.render(root, &map, &options)
        })
        .await;
        ToolOutput::text(fail_on!(fail_on!(output)))
    }

    // ------------------------------------------------------------------ get_review

    pub async fn get_review(
        &self,
        session: &Arc<Session>,
        arguments: GetReviewArguments,
    ) -> ToolOutput {
        let root = arguments.root.as_deref().map(real_path);
        let Some(found) = session.resolve_review(arguments.review_id.as_deref(), root.as_deref())
        else {
            return ToolOutput::fail("No review found.");
        };
        if let Err(error) = self.services.bundles.resolve_anchors(&found.id) {
            log::warn!("[co-review] re-anchoring {} failed: {error:#}", found.id);
        }
        let review = self.services.store.get(&found.id).unwrap_or(found);
        // Which parts of the repository the reviewer has looked at, so the agent can point at what's left.
        let coverage = if review.bundle.is_none() {
            let (workspace, viewed) = (uri_to_path(&review.workspace_root), review.viewed.clone());
            tokio::task::spawn_blocking(move || {
                let index = review_index::RepoIndex::new();
                let map = index.build(Path::new(&workspace))?;
                let viewed = viewed
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(path, at)| (path, at as f64))
                    .collect();
                anyhow::Ok(index.coverage(&map, &viewed))
            })
            .await
            .ok()
            .and_then(Result::ok)
        } else {
            None
        };
        ToolOutput::json(
            &Object::new()
                .spread(self.batch(&review))
                .maybe("coverage", coverage)
                .set(
                    "threads",
                    review
                        .threads
                        .iter()
                        .map(|thread| describe(&review, thread))
                        .collect::<Vec<_>>(),
                )
                .build(),
        )
    }

    // ------------------------------------------------------------------ channel

    /// Claude Code channels: pushes the reviewer's questions and submissions into the agent's session as they happen,
    /// so it answers without polling. Only for Claude Code clients; a session not started with channels ignores the
    /// notifications. Pushed questions are not marked delivered, so await_reviewer still returns them in that case.
    fn start_channel(self: &Arc<Self>, session: &Arc<Session>, opened: &Review) {
        let mut channel = session.channel.lock();
        let Some(channel) = channel.as_mut() else {
            return;
        };
        if let Some(task) = channel.task.take() {
            task.abort();
        }
        let claude = session
            .client_name
            .lock()
            .as_deref()
            .is_some_and(|name| name.to_lowercase().contains("claude"));
        if !claude {
            return;
        }
        let reviewer = Arc::clone(self);
        let sink = Arc::clone(&channel.sink);
        let weak_session = Arc::downgrade(session);
        let review_id = opened.id.clone();
        let changes = self.services.store.subscribe_changes();
        let mut pushed = HashSet::new();
        let mut submits = opened.verdict.as_ref().map_or(0, |verdict| verdict.count);
        let mut told_closed = None;
        let task = tokio::spawn(async move {
            let mut current = reviewer.services.store.get(&review_id);
            loop {
                let Some(session) = weak_session.upgrade() else {
                    return;
                };
                if let Some(review) = current.take() {
                    let agent = session.agent();
                    for thread in reviewer.waiting_threads(&review, &agent, &session) {
                        let Some(last) = thread.messages.last() else {
                            continue;
                        };
                        if pushed.insert(last.id.clone()) {
                            let meta = meta(&[
                                ("review_id", &review.id),
                                ("thread_id", &thread_ref(&thread)),
                                ("event", "comment"),
                            ]);
                            sink.notify(channel_text(&review, &thread), meta);
                        }
                    }
                    if let Some(closed) = &review.closed
                        && told_closed != Some(closed.at)
                    {
                        told_closed = Some(closed.at);
                        sink.notify(
                            "The reviewer closed the review window. Call await_reviewer (it returns at once) for what to do.".to_string(),
                            meta(&[("review_id", &review.id), ("event", "closed")]),
                        );
                    }
                    if let Some(verdict) = review
                        .verdict
                        .as_ref()
                        .filter(|verdict| verdict.count > submits)
                    {
                        submits = verdict.count;
                        let summary = if verdict.summary.is_empty() {
                            String::new()
                        } else {
                            format!(" \u{2014} \"{}\"", verdict.summary)
                        };
                        sink.notify(
                            format!(
                                "The reviewer submitted round {}: {}{summary}. Call await_reviewer (it returns immediately) for the open comments and accepted suggestions.",
                                verdict.count,
                                name_of(&verdict.decision),
                            ),
                            meta(&[("review_id", &review.id), ("event", "submitted")]),
                        );
                    }
                }
                drop(session);
                // Each change as it was made (as the TypeScript's listener sees it), not the latest: a write that
                // follows (e.g. the question's intent after its message) doesn't change what was pushed for it.
                while current.is_none() {
                    match changes.recv().await {
                        Ok(ReviewChange::Changed { review }) if review.id == review_id => {
                            current = Some(review)
                        }
                        Ok(_) => {}
                        Err(_) => return,
                    }
                }
            }
        });
        channel.task = Some(task.abort_handle());
    }
}

fn meta(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), json!(value)))
        .collect()
}

/// A thread as a channel message: where it is, the code, and the conversation.
fn channel_text(review: &Review, thread: &ReviewThread) -> String {
    let described = describe(review, thread);
    let location = &described["location"];
    let path = location["path"].as_str().unwrap_or("");
    let start = location["startLine"].as_u64().filter(|line| *line != 0);
    let end = location["endLine"].as_u64();
    let place = if path.is_empty() {
        "the repository".to_string()
    } else {
        match (start, end) {
            (Some(start), Some(end)) if end != start && end != 0 => format!("{path}:{start}-{end}"),
            (Some(start), _) => format!("{path}:{start}"),
            _ => path.to_string(),
        }
    };
    let symbol = location["symbol"]
        .as_str()
        .map(|symbol| format!(" ({symbol})"))
        .unwrap_or_default();
    let question = if thread.intent == ThreadIntent::Question {
        ", a question for you"
    } else {
        ""
    };
    let mut parts = vec![format!("#{} on {place}{symbol}{question}", thread.number)];
    if let Some(code) = described["code"].as_str().filter(|code| !code.is_empty()) {
        parts.push(
            code.split('\n')
                .map(|line| format!("> {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    for message in described["messages"].as_array().into_iter().flatten() {
        let text = format!(
            "{}: {}",
            message["author"].as_str().unwrap_or(""),
            message["body"].as_str().unwrap_or("")
        );
        parts.push(text);
    }
    parts.push(format!(
        "Answer with reply({{ threadId: \"{}\" }}). {ANSWER_STYLE_SHORT}",
        thread_ref(thread)
    ));
    parts.retain(|part| !part.is_empty());
    parts.join("\n\n")
}

/// What an agent waiting on a review hears when the reviewer closed it.
fn closed_notice(review: &Review) -> Value {
    json!({
        "status": "closed",
        "reviewId": review.id,
        "by": "reviewer",
        "hint": "The reviewer closed the review window (or quit Co-Review); the review and its threads are saved. Stop waiting on this review. Tell the user in the conversation what is still open, if anything. To continue, open_review({ reviewId }) shows it again.",
    })
}

fn count(review: &Review, status: ThreadStatus) -> usize {
    review
        .threads
        .iter()
        .filter(|thread| thread.status == status)
        .count()
}

fn is_markdown(page: &str) -> bool {
    let lower = page.to_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

/// The JSON name of an enum value (`request-changes`, `high`).
fn name_of(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// `new Date(ms).toISOString()`.
fn iso_time(millis: u64) -> String {
    chrono::DateTime::from_timestamp_millis(millis as i64)
        .map(|time| time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_default()
}

fn location(kind: LocationKind) -> CodeLocation {
    CodeLocation {
        kind,
        uri: None,
        range: None,
        symbol: None,
        anchor: None,
        doc_anchor: None,
        patch_anchor: None,
        tracked: None,
        extra: Map::new(),
    }
}

/// A file or folder of the repository (`.` or `/`: the repository itself).
fn path_location(root: &str, file: &str) -> CodeLocation {
    if file == root {
        return location(LocationKind::Repository);
    }
    let kind = if Path::new(file).is_dir() {
        LocationKind::Directory
    } else {
        LocationKind::File
    };
    CodeLocation {
        uri: Some(path_to_file_uri(file)),
        ..location(kind)
    }
}

/// The document's design-format check, so the agent learns about departures in the loop.
fn format_of(document: Option<&str>) -> Map<String, Value> {
    let Some(document) = document else {
        return Map::new();
    };
    let Ok(text) = std::fs::read_to_string(document) else {
        return Map::new();
    };
    let mut format = review_design::document_format(&text, &basename(document));
    if !format.declared && format.mode == review_design::Mode::Design {
        format
            .warnings
            .push("Looks like a design document: add frontmatter `co-review: design` to declare (and check) the structure.".to_string());
    }
    let mut value = Object::new().set("format", &format).into_map();
    if !format.warnings.is_empty()
        && let Some(Value::Object(format)) = value.get_mut("format")
    {
        format.insert(
            "next".to_string(),
            json!("Fix each warning in the document and call open_review({ dir, open: false }) again; when there are none, show it with open_review({ reviewId })."),
        );
    }
    value
}

/// Starts an agent message carrying buttons (a permission request), done at once; returns its id.
fn start_permission_message(
    reviewer: &CoReviewer,
    review_id: &str,
    thread_id: &str,
    author: Participant,
    body: String,
    permission: PermissionRequest,
) -> anyhow::Result<String> {
    let init = MessagePatch {
        body: Some(body),
        status: Some(MessageStatus::Done),
        permission: Some(permission),
        ..MessagePatch::default()
    };
    Ok(reviewer
        .services
        .store
        .start_message(review_id, thread_id, author, init)?)
}

fn update_permission(
    reviewer: &CoReviewer,
    review_id: &str,
    thread_id: &str,
    message_id: &str,
    permission: PermissionRequest,
) -> anyhow::Result<()> {
    let patch = MessagePatch {
        permission: Some(permission),
        ..MessagePatch::default()
    };
    reviewer
        .services
        .store
        .update_message(review_id, thread_id, message_id, patch)?;
    Ok(())
}
