//! What the UI hears from the agents: thread updates and connection state, on std channels.

use std::sync::Arc;
use std::sync::mpsc;

use parking_lot::Mutex;
use review_mcp::HumanDecisions;
use review_model::{AgentState, ReviewThread};
use review_store::{MessagePatch, ReviewStore};

use crate::turn::Turn;

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum AgentEvent {
    /// The agent changed a thread: its streaming answer, activity, permission request or agent state.
    Thread {
        review_id: String,
        thread: ReviewThread,
    },
    /// The review's agent process changed state.
    Connection {
        review_id: String,
        state: ConnectionState,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// The process is starting and being initialized.
    Starting,
    /// Initialized; questions can be sent.
    Ready,
    /// It could not be started or initialized.
    Failed { message: String },
    /// The process ended (`code` is `None` when it was killed by a signal).
    Exited { code: Option<i32> },
}

/// The store, decisions and listeners shared by the service and its connections.
pub(crate) struct Core {
    pub store: Arc<ReviewStore>,
    pub decisions: Arc<HumanDecisions>,
    listeners: Mutex<Vec<mpsc::Sender<AgentEvent>>>,
}

impl Core {
    pub fn new(store: Arc<ReviewStore>, decisions: Arc<HumanDecisions>) -> Self {
        Self {
            store,
            decisions,
            listeners: Mutex::new(Vec::new()),
        }
    }

    pub fn subscribe(&self) -> mpsc::Receiver<AgentEvent> {
        let (sender, receiver) = mpsc::channel();
        self.listeners.lock().push(sender);
        receiver
    }

    pub fn emit(&self, event: AgentEvent) {
        self.listeners
            .lock()
            .retain(|listener| listener.send(event.clone()).is_ok());
    }

    pub fn thread_changed(&self, review_id: &str, thread: ReviewThread) {
        self.emit(AgentEvent::Thread {
            review_id: review_id.to_string(),
            thread,
        });
    }

    pub fn connection_changed(&self, review_id: &str, state: ConnectionState) {
        self.emit(AgentEvent::Connection {
            review_id: review_id.to_string(),
            state,
        });
    }

    pub fn set_agent_state(
        &self,
        review_id: &str,
        thread_id: &str,
        state: AgentState,
    ) {
        match self.store.set_agent_state(review_id, thread_id, state) {
            Ok(thread) => self.thread_changed(review_id, thread),
            Err(error) => {
                log::error!("agent state of thread {thread_id} not saved: {error}")
            }
        }
    }

    /// Writes the turn's message as it stands (body, activity, permission) into the thread.
    pub fn flush(&self, turn: &Mutex<Turn>) {
        let (review_id, thread_id, message_id, patch) = {
            let mut turn = turn.lock();
            turn.dirty = false;
            let patch = MessagePatch {
                body: Some(turn.body.clone()),
                activity: Some(turn.activity.clone()),
                permission: turn.permission.clone(),
                ..MessagePatch::default()
            };
            (
                turn.review_id.clone(),
                turn.thread_id.clone(),
                turn.message_id.clone(),
                patch,
            )
        };
        match self
            .store
            .update_message(&review_id, &thread_id, &message_id, patch)
        {
            Ok(thread) => self.thread_changed(&review_id, thread),
            Err(error) => {
                log::error!("agent message in thread {thread_id} not saved: {error}")
            }
        }
    }
}
