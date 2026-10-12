//! What agents wait for from the reviewer: decisions (buttons in a thread) and whether an agent is listening.
//! Port of `agent-coordination.ts`. Both are executor-agnostic, so the app (GPUI) and the MCP server (tokio) share
//! them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use review_model::AgentPresence;

/// Decisions an agent is waiting for (`ask_reviewer` questions, `post_review_to_github` confirmations, ACP
/// permission requests). The reviewer answers them from the thread; whoever asked is resumed.
#[derive(Default)]
pub struct HumanDecisions {
    pending: Mutex<HashMap<String, smol::channel::Sender<Option<String>>>>,
}

impl HumanDecisions {
    /// The chosen option id, or `None` when cancelled or not answered within `timeout`.
    pub async fn wait(&self, request_id: &str, timeout: Duration) -> Option<String> {
        let (sender, receiver) = smol::channel::bounded(1);
        self.pending.lock().insert(request_id.to_string(), sender);
        let answered = async { receiver.recv().await.ok().flatten() };
        let answer = smol::future::or(answered, async {
            smol::Timer::after(timeout).await;
            None
        })
        .await;
        self.pending.lock().remove(request_id);
        answer
    }

    /// Answers a decision (`None` cancels it). Unknown ids are ignored: the wait may have timed out.
    pub fn resolve(&self, request_id: &str, option_id: Option<String>) {
        if let Some(sender) = self.pending.lock().remove(request_id) {
            sender.try_send(option_id).ok();
        }
    }
}

/// Whether an MCP-attached agent is currently waiting for the reviewer, per review.
#[derive(Default)]
pub struct AgentPresenceTracker {
    presence: Mutex<HashMap<String, Presence>>,
    listeners: Mutex<Vec<smol::channel::Sender<AgentPresence>>>,
}

struct Presence {
    listening: bool,
    last_seen: u64,
    waiters: u32,
}

impl AgentPresenceTracker {
    pub fn get(&self, review_id: &str) -> Option<AgentPresence> {
        self.presence
            .lock()
            .get(review_id)
            .map(|presence| AgentPresence {
                review_id: review_id.to_string(),
                listening: presence.listening,
                last_seen: presence.last_seen,
            })
    }

    /// Every change of presence, for the review panel.
    pub fn subscribe(&self) -> smol::channel::Receiver<AgentPresence> {
        let (sender, receiver) = smol::channel::unbounded();
        self.listeners.lock().push(sender);
        receiver
    }

    /// Marks the agent as listening on `review_id` until the returned guard is dropped.
    pub fn listen(self: &Arc<Self>, review_id: &str) -> Listening {
        self.update(review_id, |presence| presence.waiters += 1);
        Listening {
            review_id: review_id.to_string(),
            tracker: Arc::clone(self),
        }
    }

    /// The agent did something on `review_id`.
    pub fn seen(&self, review_id: &str) {
        self.update(review_id, |_| {});
    }

    fn update(&self, review_id: &str, change: impl FnOnce(&mut Presence)) {
        let event = {
            let mut presence = self.presence.lock();
            let entry = presence.entry(review_id.to_string()).or_insert(Presence {
                listening: false,
                last_seen: 0,
                waiters: 0,
            });
            change(entry);
            entry.listening = entry.waiters > 0;
            entry.last_seen = now_millis();
            AgentPresence {
                review_id: review_id.to_string(),
                listening: entry.listening,
                last_seen: entry.last_seen,
            }
        };
        self.listeners
            .lock()
            .retain(|listener| listener.try_send(event.clone()).is_ok());
    }
}

/// Keeps an agent shown as listening; dropping it stops.
pub struct Listening {
    review_id: String,
    tracker: Arc<AgentPresenceTracker>,
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.tracker.update(&self.review_id, |presence| {
            presence.waiters = presence.waiters.saturating_sub(1)
        });
    }
}

pub(crate) fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions_resolve_and_time_out() {
        let decisions = HumanDecisions::default();
        // The wait registers on its first poll, before the resolve runs.
        let (answer, ()) = smol::block_on(smol::future::zip(
            decisions.wait("request", Duration::from_secs(10)),
            async { decisions.resolve("request", Some("1".to_string())) },
        ));
        assert_eq!(answer, Some("1".to_string()));
        let timed_out = smol::block_on(decisions.wait("other", Duration::from_millis(10)));
        assert_eq!(timed_out, None);
        assert!(decisions.pending.lock().is_empty());
    }

    #[test]
    fn listening_while_guarded() {
        let tracker = Arc::new(AgentPresenceTracker::default());
        let events = tracker.subscribe();
        let first = tracker.listen("review");
        let second = tracker.listen("review");
        drop(first);
        assert!(
            tracker
                .get("review")
                .is_some_and(|presence| presence.listening)
        );
        drop(second);
        assert!(
            tracker
                .get("review")
                .is_some_and(|presence| !presence.listening)
        );
        let listening: Vec<bool> = std::iter::from_fn(|| events.try_recv().ok())
            .map(|presence| presence.listening)
            .collect();
        assert_eq!(listening, vec![true, true, true, false]);
    }
}
