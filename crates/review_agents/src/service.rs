//! Connects reviews to ACP agents. Co-Review is the ACP *client*: it launches the agent, sends it the questions asked
//! in review threads, and streams its answers, tool activity and permission requests back into the same threads.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Weak};
use std::time::Duration;

use agent_client_protocol::schema::v1::StopReason;
use anyhow::{Context as _, anyhow};
use parking_lot::Mutex;
use review_mcp::HumanDecisions;
use review_model::{
    AgentConfig, AgentState, MessageStatus, Participant, ParticipantKind, Review,
    ReviewThread, ThreadIntent,
};
use review_store::{MessagePatch, ReviewChange, ReviewStore};

use crate::connection::Connection;
use crate::events::{AgentEvent, Core};
use crate::presets::{is_acp, same_process};
use crate::prompt::{
    MAP_WAIT, build_prompt, repository_map, review_root, thread_file,
    warm_repository_map,
};
use crate::settings::{AgentSetting, offered_settings, settings_label};
use crate::turn::Turn;

/// How often a streaming answer is written into its thread.
const FLUSH_INTERVAL: Duration = Duration::from_millis(250);

/// The ACP agents of all reviews: one process per review, one session per thread.
///
/// Every method returns quickly except [`AgentService::list_options`], which waits for the agent to start; call it
/// off the UI thread. Progress arrives on [`AgentService::subscribe`].
pub struct AgentService {
    core: Arc<Core>,
    this: Weak<AgentService>,
    /// reviewId -> the running agent
    connections: Mutex<HashMap<String, Arc<Connection>>>,
    /// threadId -> the queue of turns for that thread, while its worker runs
    queues: Mutex<HashMap<String, mpsc::Sender<String>>>,
}

/// The participant an ACP agent writes as.
pub fn agent_participant(agent: &AgentConfig) -> Participant {
    Participant {
        id: format!("agent:{}", agent.id),
        kind: ParticipantKind::Agent,
        name: agent.name.clone(),
    }
}

/// Whether the review's ACP agent should answer the thread's latest message: a question, or a follow-up in a thread
/// the agent is part of.
pub fn answers(review: &Review, thread: &ReviewThread) -> bool {
    let Some(agent) = review.agent.as_ref().filter(|agent| is_acp(Some(agent)))
    else {
        return false;
    };
    if thread
        .messages
        .last()
        .is_none_or(|message| message.author.kind != ParticipantKind::Human)
    {
        return false;
    }
    let participant = agent_participant(agent).id;
    thread.intent == ThreadIntent::Question
        || thread
            .messages
            .iter()
            .any(|message| message.author.id == participant)
}

impl AgentService {
    /// `decisions` is shared with the MCP server, so the reviewer answers a permission request the same way as an
    /// `ask_reviewer` question: [`HumanDecisions::resolve`] with the permission request's id and an option id.
    pub fn new(
        store: Arc<ReviewStore>,
        decisions: Arc<HumanDecisions>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|this| Self {
            core: Arc::new(Core::new(store, decisions)),
            this: this.clone(),
            connections: Mutex::default(),
            queues: Mutex::default(),
        })
    }

    /// Thread updates and connection state, from now on.
    pub fn subscribe(&self) -> mpsc::Receiver<AgentEvent> {
        self.core.subscribe()
    }

    /// Makes `agent` the review's agent (keeping the chosen settings when it is the same agent), starts it, opens a
    /// session ahead of the first question and builds the repository map, all in the background.
    pub fn connect(
        &self,
        review_id: &str,
        mut agent: AgentConfig,
    ) -> anyhow::Result<Review> {
        let review = self.review(review_id)?;
        if !is_acp(Some(&agent)) {
            return Err(anyhow!("{} has no ACP command", agent.name));
        }
        if let Some(previous) = &review.agent
            && same_process(previous, &agent)
            && agent.settings.is_none()
        {
            agent.settings = previous.settings.clone();
            agent.settings_label = previous.settings_label.clone();
        }
        let review = self.core.store.set_agent(review_id, Some(agent))?;
        self.warm(&review);
        Ok(review)
    }

    /// Stops the review's agent. The review keeps it as its agent; the next question starts it again.
    pub fn disconnect(&self, review_id: &str) {
        let connection = self.connections.lock().remove(review_id);
        if let Some(connection) = connection {
            connection.stop();
        }
    }

    /// Stops every agent (when the app quits).
    pub fn shutdown(&self) {
        let connections: Vec<_> = self
            .connections
            .lock()
            .drain()
            .map(|(_, connection)| connection)
            .collect();
        for connection in connections {
            connection.stop();
        }
    }

    /// The model and reasoning-effort choices the review's agent offers, model first, with the review's choice as
    /// `current`. Starts the agent if needed and waits for its first session.
    pub fn list_options(
        &self,
        review_id: &str,
    ) -> anyhow::Result<Vec<AgentSetting>> {
        let review = self.review(review_id)?;
        let connection = self.connection(&review)?;
        let options = connection.spare_options()?;
        let chosen = review
            .agent
            .as_ref()
            .and_then(|agent| agent.settings.as_ref());
        Ok(offered_settings(&options, chosen))
    }

    /// Chooses `value` for the agent's option `option_id`, stored in `review.agent.settings`. Every session of the
    /// review, open or new, gets it before its next prompt.
    pub fn set_option(
        &self,
        review_id: &str,
        option_id: &str,
        value: &str,
    ) -> anyhow::Result<Review> {
        let review = self.review(review_id)?;
        let mut agent = review
            .agent
            .clone()
            .filter(|agent| is_acp(Some(agent)))
            .context("The review has no ACP agent")?;
        let mut settings = agent.settings.clone().unwrap_or_default();
        settings.insert(option_id.to_string(), value.to_string());
        let offered = self
            .connections
            .lock()
            .get(review_id)
            .and_then(|connection| connection.spare_options().ok())
            .map(|options| offered_settings(&options, None))
            .unwrap_or_default();
        agent.settings_label = Some(settings_label(&offered, &settings))
            .filter(|label| !label.is_empty());
        agent.settings = Some(settings);
        let review = self.core.store.set_agent(review_id, Some(agent.clone()))?;
        self.use_settings(review_id, agent);
        Ok(review)
    }

    /// Queues a turn for the thread: the agent answers the latest reviewer messages, after any turn already queued
    /// for the thread.
    pub fn ask(&self, review_id: &str, thread_id: &str) {
        let mut queues = self.queues.lock();
        if let Some(queue) = queues.get(thread_id)
            && queue.send(review_id.to_string()).is_ok()
        {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        if sender.send(review_id.to_string()).is_err() {
            return;
        }
        queues.insert(thread_id.to_string(), sender);
        drop(queues);
        let this = self.this.clone();
        let worker_thread_id = thread_id.to_string();
        let spawned = std::thread::Builder::new()
            .name("co-review-agent-turn".to_string())
            .spawn(move || {
                loop {
                    let Some(service) = this.upgrade() else {
                        return;
                    };
                    // The queue is checked and removed under its lock, so an ask either lands here or starts a worker.
                    let next = {
                        let mut queues = service.queues.lock();
                        match receiver.try_recv() {
                            Ok(review_id) => Some(review_id),
                            Err(_) => {
                                queues.remove(&worker_thread_id);
                                None
                            }
                        }
                    };
                    let Some(review_id) = next else {
                        return;
                    };
                    service.run_turn(&review_id, &worker_thread_id);
                }
            });
        if let Err(error) = spawned {
            log::error!("could not start an agent turn: {error}");
            self.queues.lock().remove(thread_id);
        }
    }

    /// Stops the thread's running turn: a pending permission request is cancelled and the agent gets
    /// `session/cancel`.
    pub fn stop(&self, review_id: &str, thread_id: &str) -> anyhow::Result<()> {
        let Some(connection) = self.connections.lock().get(review_id).cloned()
        else {
            return Ok(());
        };
        let Some(session_id) = connection.sessions.lock().get(thread_id).cloned()
        else {
            return Ok(());
        };
        if let Some(request_id) = connection.pending_permission(&session_id) {
            self.core.decisions.resolve(&request_id, None);
        }
        connection.cancel(&session_id)
    }

    /// Follows the store: a deleted review's agent stops; another agent replaces the running one; changed settings
    /// apply to the next prompts; an ACP agent set on a review starts ahead of its first question.
    pub fn watch_store(&self) -> anyhow::Result<()> {
        let changes = self.core.store.subscribe_changes();
        let this = self.this.clone();
        std::thread::Builder::new()
            .name("co-review-agent-watch".to_string())
            .spawn(move || {
                while let Ok(change) = changes.recv_blocking() {
                    let Some(service) = this.upgrade() else {
                        return;
                    };
                    service.store_changed(change);
                }
            })?;
        Ok(())
    }

    fn store_changed(&self, change: ReviewChange) {
        let review = match change {
            ReviewChange::Deleted { review_id, .. } => {
                self.disconnect(&review_id);
                return;
            }
            ReviewChange::Changed { review } => review,
        };
        let connection = self.connections.lock().get(&review.id).cloned();
        match (&connection, &review.agent) {
            (Some(connection), Some(agent))
                if same_process(&connection.config.lock(), agent) =>
            {
                if connection.config.lock().settings != agent.settings {
                    self.use_settings(&review.id, agent.clone());
                }
            }
            (Some(_), _) => self.disconnect(&review.id),
            (None, _) => {}
        }
        if is_acp(review.agent.as_ref())
            && !self.connections.lock().contains_key(&review.id)
        {
            self.warm(&review);
        }
    }

    fn review(&self, review_id: &str) -> anyhow::Result<Review> {
        self.core
            .store
            .get(review_id)
            .with_context(|| format!("Unknown review {review_id}"))
    }

    /// Starts the agent and the repository map, so the first question doesn't wait for the agent to launch (`npx`
    /// alone can take seconds) or the map to build.
    fn warm(&self, review: &Review) {
        match self.connection(review) {
            Ok(connection) => connection.open_spare(),
            Err(error) => log::warn!("agent not started: {error:#}"),
        }
        warm_repository_map(&review_root(review));
    }

    /// The review's running agent, started if needed (or restarted when the review now has another agent).
    fn connection(&self, review: &Review) -> anyhow::Result<Arc<Connection>> {
        let agent = review
            .agent
            .clone()
            .filter(|agent| is_acp(Some(agent)))
            .context("The review has no ACP agent")?;
        let mut connections = self.connections.lock();
        if let Some(existing) = connections.get(&review.id).cloned() {
            if !existing.has_exited()
                && same_process(&existing.config.lock(), &agent)
            {
                *existing.config.lock() = agent;
                return Ok(existing);
            }
            connections.remove(&review.id);
            existing.stop();
        }
        let this = self.this.clone();
        let connection = Connection::start(
            Arc::clone(&self.core),
            &review.id,
            agent,
            review_root(review),
            move |connection| {
                // A command that exited is forgotten, so the next question starts it again.
                if let Some(service) = this.upgrade() {
                    let mut connections = service.connections.lock();
                    if connections
                        .get(&connection.review_id)
                        .is_some_and(|current| Arc::ptr_eq(current, connection))
                    {
                        connections.remove(&connection.review_id);
                    }
                }
            },
        )?;
        connections.insert(review.id.clone(), Arc::clone(&connection));
        Ok(connection)
    }

    /// The review's agent with new settings: each session gets them before its next prompt.
    fn use_settings(&self, review_id: &str, agent: AgentConfig) {
        if let Some(connection) = self.connections.lock().get(review_id) {
            *connection.config.lock() = agent;
        }
    }

    fn run_turn(&self, review_id: &str, thread_id: &str) {
        let Some(review) = self.core.store.get(review_id) else {
            return;
        };
        let Some(thread) = review
            .threads
            .iter()
            .find(|thread| thread.id == thread_id)
            .cloned()
        else {
            return;
        };
        let Some(agent) = review.agent.clone().filter(|agent| is_acp(Some(agent)))
        else {
            return;
        };
        let init = MessagePatch {
            status: Some(MessageStatus::Streaming),
            activity: Some(Vec::new()),
            ..MessagePatch::default()
        };
        let message_id = match self.core.store.start_message(
            review_id,
            thread_id,
            agent_participant(&agent),
            init,
        ) {
            Ok(message_id) => message_id,
            Err(error) => {
                log::error!("agent turn not started: {error}");
                return;
            }
        };
        self.core
            .set_agent_state(review_id, thread_id, AgentState::Working);
        let turn =
            Arc::new(Mutex::new(Turn::new(review_id, thread_id, &message_id)));
        let mut connection = None;
        let outcome = self.prompt(&review, &thread, &turn, &mut connection);
        let mut turn = turn.lock().clone();
        let status = match outcome {
            Ok(StopReason::Cancelled) => MessageStatus::Cancelled,
            Ok(stop_reason) => {
                if stop_reason == StopReason::Refusal {
                    turn.body.push_str(if turn.body.is_empty() {
                        "_The agent declined to answer._"
                    } else {
                        "\n\n_(The agent declined to continue.)_"
                    });
                }
                MessageStatus::Done
            }
            Err(error) => {
                let details = connection
                    .as_ref()
                    .map(|connection| connection.stderr_tail())
                    .unwrap_or_default();
                let separator = if turn.body.is_empty() { "" } else { "\n\n" };
                turn.body
                    .push_str(&format!("{separator}**Agent error:** {error:#}"));
                if !details.is_empty() {
                    turn.body.push_str(&format!("\n\n```\n{details}\n```"));
                }
                MessageStatus::Error
            }
        };
        if turn.body.trim().is_empty() && status == MessageStatus::Done {
            turn.body = "_(No answer.)_".to_string();
        }
        let patch = MessagePatch {
            body: Some(turn.body),
            status: Some(status),
            activity: Some(turn.activity),
            permission: turn.permission,
            ..MessagePatch::default()
        };
        match self.core.store.update_message(
            review_id,
            thread_id,
            &message_id,
            patch,
        ) {
            Ok(thread) => self.core.thread_changed(review_id, thread),
            Err(error) => log::error!("agent answer not saved: {error}"),
        }
        self.core
            .set_agent_state(review_id, thread_id, AgentState::Idle);
    }

    /// Sends the turn's prompt in the thread's session (opened, with the review's settings, on its first question)
    /// and streams the answer into the thread until the agent stops.
    fn prompt(
        &self,
        review: &Review,
        thread: &ReviewThread,
        turn: &Arc<Mutex<Turn>>,
        used: &mut Option<Arc<Connection>>,
    ) -> anyhow::Result<StopReason> {
        let connection = self.connection(review)?;
        *used = Some(Arc::clone(&connection));
        smol::block_on(connection.acp())?;
        let existing = connection.sessions.lock().get(&thread.id).cloned();
        let fresh = existing.is_none();
        let session_id = match existing {
            Some(session_id) => session_id,
            None => {
                let session_id = connection.take_session()?;
                connection
                    .sessions
                    .lock()
                    .insert(thread.id.clone(), session_id.clone());
                session_id
            }
        };
        // The review's settings, applied where the session differs (a new session, or new settings).
        if let Err(error) = smol::block_on(connection.apply_settings(&session_id)) {
            log::error!("agent settings failed: {error:#}");
        }
        connection.begin_turn(&session_id, Arc::clone(turn));
        let map = fresh
            .then(|| {
                let root = review_root(review);
                let focus = thread_file(&root, thread).map(|(_, relative)| relative);
                repository_map(&root, focus, MAP_WAIT)
            })
            .flatten();
        let prompt = build_prompt(review, thread, fresh, map);
        let response = smol::block_on(smol::future::or(
            connection.prompt(&session_id, prompt),
            async {
                loop {
                    smol::Timer::after(FLUSH_INTERVAL).await;
                    if turn.lock().dirty {
                        self.core.flush(turn);
                    }
                }
            },
        ));
        if response.is_err() {
            // When the process died, say so before the error (the exit is noted on the running turn).
            connection.wait_for_exit(Duration::from_secs(1));
        }
        connection.end_turn(&session_id);
        Ok(response?.stop_reason)
    }
}

impl Drop for AgentService {
    fn drop(&mut self) {
        self.shutdown();
    }
}
