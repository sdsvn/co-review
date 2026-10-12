//! The core review store: persistence, mutations, locking and change events.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use review_model::{
    ActivityKind, AgentConfig, AgentState, AgentTransport, CodeLocation, Participant,
    ReviewActivity, ReviewBundle, ReviewDecision, ReviewMessage, ReviewScope, ReviewThread,
    ReviewVerdict, ThreadIntent, ThreadStatus, VerdictToAgent,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::file_uri::{file_uri_to_path, path_to_file_uri, real_uri};
use crate::snapshots;

// Re-export the Review type from review_model
pub use review_model::Review;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ReviewStoreError {
    UnknownReview(String),
    UnknownThread { review_id: String, thread_id: String },
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for ReviewStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownReview(id) => write!(f, "unknown review {id}"),
            Self::UnknownThread {
                review_id,
                thread_id,
            } => write!(f, "unknown thread {thread_id} in review {review_id}"),
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Json(e) => write!(f, "json error: {e}"),
        }
    }
}

impl std::error::Error for ReviewStoreError {}

impl From<std::io::Error> for ReviewStoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for ReviewStoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

// ---------------------------------------------------------------------------
// Change events
// ---------------------------------------------------------------------------

/// Broadcast when a review changes or is deleted.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum ReviewChange {
    Changed { review: Review },
    Deleted { review_id: String, workspace_root: String },
}

/// Broadcast when a message is added to a thread.
#[derive(Debug, Clone)]
pub struct MessageAdded {
    pub review: Review,
    pub thread: ReviewThread,
    pub message: ReviewMessage,
}

// ---------------------------------------------------------------------------
// CreateReviewParams
// ---------------------------------------------------------------------------

pub struct CreateReviewParams {
    pub workspace_root: String,
    pub title: String,
    pub scope: ReviewScope,
    pub author: Participant,
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Per-review lock: held during the full read-modify-write-broadcast cycle.
type ReviewLock = Arc<Mutex<()>>;

struct StoreInner {
    /// reviewId -> file path, populated lazily.
    locations: HashMap<String, PathBuf>,
    /// Per-review mutation locks.
    locks: HashMap<String, ReviewLock>,
    /// Change event subscribers.
    change_senders: Vec<smol::channel::Sender<ReviewChange>>,
    /// Message event subscribers.
    message_senders: Vec<smol::channel::Sender<MessageAdded>>,
}

// ---------------------------------------------------------------------------
// ReviewStore
// ---------------------------------------------------------------------------

/// Local, file-based review persistence.
///
/// Review state lives outside the repository:
/// `$CO_REVIEW_HOME` (default `~/.co-review`) / workspaces / <hash of root URI> / reviews / <id>.json
///
/// Thread-safe: mutations are serialized per review via internal locking.
/// Executor-agnostic: no tokio, no GPUI; works from std threads and smol tasks.
pub struct ReviewStore {
    /// Override for `co_review_home()`; when set, all paths are under this directory.
    home: Option<PathBuf>,
    inner: Mutex<StoreInner>,
}

impl ReviewStore {
    pub fn new() -> Self {
        Self {
            home: None,
            inner: Mutex::new(StoreInner {
                locations: HashMap::new(),
                locks: HashMap::new(),
                change_senders: Vec::new(),
                message_senders: Vec::new(),
            }),
        }
    }

    /// Create a store rooted at a specific home directory (for testing).
    pub fn with_home(home: PathBuf) -> Self {
        Self {
            home: Some(home),
            inner: Mutex::new(StoreInner {
                locations: HashMap::new(),
                locks: HashMap::new(),
                change_senders: Vec::new(),
                message_senders: Vec::new(),
            }),
        }
    }

    fn home(&self) -> PathBuf {
        self.home.clone().unwrap_or_else(crate::co_review_home)
    }

    // ----- path helpers -----

    /// Where the reviews (and other state) of a workspace live, outside the repository.
    pub fn workspace_dir(&self, workspace_root: &str) -> PathBuf {
        let uri = real_uri(workspace_root);
        let hash = format!("{:x}", Sha256::digest(uri.as_bytes()));
        let prefix = &hash[..16];
        self.home().join("workspaces").join(prefix)
    }

    /// The directory holding review JSON files for a workspace.
    fn reviews_dir(&self, workspace_root: &str) -> PathBuf {
        self.workspace_dir(workspace_root).join("reviews")
    }

    // ----- subscriptions -----

    /// Subscribe to review change events. Returns a channel receiver.
    pub fn subscribe_changes(&self) -> smol::channel::Receiver<ReviewChange> {
        let (sender, receiver) = smol::channel::unbounded();
        self.inner.lock().change_senders.push(sender);
        receiver
    }

    /// Subscribe to message-added events. Returns a channel receiver.
    pub fn subscribe_messages(&self) -> smol::channel::Receiver<MessageAdded> {
        let (sender, receiver) = smol::channel::unbounded();
        self.inner.lock().message_senders.push(sender);
        receiver
    }

    fn fire_change(&self, change: ReviewChange) {
        let mut inner = self.inner.lock();
        inner
            .change_senders
            .retain(|s| s.try_send(change.clone()).is_ok());
    }

    fn fire_message(&self, event: MessageAdded) {
        let mut inner = self.inner.lock();
        inner
            .message_senders
            .retain(|s| s.try_send(event.clone()).is_ok());
    }

    // ----- location tracking -----

    fn remember_location(&self, review_id: &str, path: PathBuf) {
        self.inner
            .lock()
            .locations
            .insert(review_id.to_string(), path);
    }

    fn get_location(&self, review_id: &str) -> Option<PathBuf> {
        self.inner.lock().locations.get(review_id).cloned()
    }

    fn remove_location(&self, review_id: &str) {
        self.inner.lock().locations.remove(review_id);
    }

    /// Find the file of a review, searching all workspaces if not already known.
    fn locate(&self, review_id: &str) -> Option<PathBuf> {
        if let Some(known) = self.get_location(review_id) {
            return Some(known);
        }
        let workspaces = self.home().join("workspaces");
        let entries = fs::read_dir(&workspaces).ok()?;
        for entry in entries.flatten() {
            let file = entry.path().join("reviews").join(format!("{review_id}.json"));
            if file.exists() {
                self.remember_location(review_id, file.clone());
                return Some(file);
            }
        }
        None
    }

    fn get_review_lock(&self, review_id: &str) -> ReviewLock {
        let mut inner = self.inner.lock();
        inner
            .locks
            .entry(review_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    // ----- reads -----

    /// List all reviews for a workspace, sorted by `updatedAt` descending.
    pub fn list(&self, workspace_root: &str) -> Vec<Review> {
        let dir = self.reviews_dir(workspace_root);
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        let mut reviews = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match fs::read_to_string(&path)
                .map_err(ReviewStoreError::from)
                .and_then(|text| serde_json::from_str::<Review>(&text).map_err(Into::into))
            {
                Ok(review) => {
                    self.remember_location(&review.id, path);
                    reviews.push(review);
                }
                Err(_) => continue,
            }
        }
        reviews.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
        reviews
    }

    /// List reviews a window should show: the workspace's own, plus change reviews
    /// whose code folder points to this workspace.
    pub fn list_for_window(&self, workspace_root: &str) -> Vec<Review> {
        let root = real_uri(workspace_root);
        let own = self.list(&root);
        let own_ids: std::collections::HashSet<&str> =
            own.iter().map(|r| r.id.as_str()).collect();
        let mut coded: Vec<Review> = self
            .list_all()
            .into_iter()
            .filter(|r| {
                if let Some(bundle) = &r.bundle
                    && let Some(code) = &bundle.code
                {
                    let code_uri = path_to_file_uri(code);
                    return code_uri == root && !own_ids.contains(r.id.as_str());
                }
                false
            })
            .collect();
        let mut all = own;
        all.append(&mut coded);
        all.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
        all
    }

    /// Every review in every workspace.
    pub fn list_all(&self) -> Vec<Review> {
        let workspaces = self.home().join("workspaces");
        let mut all = Vec::new();
        let entries = match fs::read_dir(&workspaces) {
            Ok(e) => e,
            Err(_) => return all,
        };
        for entry in entries.flatten() {
            let info_path = entry.path().join("workspace.json");
            let info_text = match fs::read_to_string(&info_path) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let info: serde_json::Value = match serde_json::from_str(&info_text) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(root) = info.get("root").and_then(|r| r.as_str()) {
                all.extend(self.list(root));
            }
        }
        all.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
        all
    }

    /// Get a single review by id.
    pub fn get(&self, review_id: &str) -> Option<Review> {
        let file = self.locate(review_id)?;
        let text = fs::read_to_string(&file).ok()?;
        serde_json::from_str(&text).ok()
    }

    // ----- writes -----

    /// Create a new review.
    pub fn create(&self, params: CreateReviewParams) -> Result<Review, ReviewStoreError> {
        let workspace_root = real_uri(&params.workspace_root);
        let now = now_millis();
        let review = Review {
            id: Uuid::new_v4().to_string(),
            title: params.title,
            workspace_root: workspace_root.clone(),
            scope: params.scope,
            participants: vec![params.author.clone()],
            threads: Vec::new(),
            activity: vec![ReviewActivity {
                kind: ActivityKind::ReviewCreated,
                actor: params.author,
                thread_id: None,
                at: now,
            }],
            next_thread_number: 1,
            created_at: now,
            updated_at: now,
            agent: None,
            bundle: None,
            overview: None,
            verdict: None,
            closed: None,
            archived_at: None,
            viewed: None,
            extra: serde_json::Map::new(),
        };
        let dir = self.reviews_dir(&workspace_root);
        fs::create_dir_all(&dir)?;
        self.write_workspace_info(&workspace_root)?;
        let file = dir.join(format!("{}.json", review.id));
        self.remember_location(&review.id, file.clone());
        atomic_write(&file, &review)?;
        self.fire_change(ReviewChange::Changed {
            review: review.clone(),
        });
        Ok(review)
    }

    /// Delete a review.
    pub fn delete(&self, review_id: &str) -> Result<(), ReviewStoreError> {
        let review = self
            .get(review_id)
            .ok_or_else(|| ReviewStoreError::UnknownReview(review_id.to_string()))?;
        let file = self
            .locate(review_id)
            .ok_or_else(|| ReviewStoreError::UnknownReview(review_id.to_string()))?;
        let _ = fs::remove_file(&file);
        self.remove_location(review_id);
        self.fire_change(ReviewChange::Deleted {
            review_id: review_id.to_string(),
            workspace_root: review.workspace_root,
        });
        Ok(())
    }

    /// Rename a review.
    pub fn rename(
        &self,
        review_id: &str,
        title: String,
    ) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, _now| {
            review.title = title;
        })
    }

    /// Archive or unarchive a review.
    pub fn archive(
        &self,
        review_id: &str,
        archived: bool,
    ) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, now| {
            review.archived_at = if archived { Some(now) } else { None };
        })
    }

    /// Create a thread in a review. Returns the updated review.
    pub fn create_thread(
        &self,
        review_id: &str,
        location: CodeLocation,
        body: String,
        author: Participant,
        options: review_model::ThreadOptions,
    ) -> Result<(Review, ReviewThread), ReviewStoreError> {
        let mut created_thread: Option<ReviewThread> = None;
        let mut message_event: Option<MessageAdded> = None;

        let review = self.mutate(review_id, |review, now| {
            let thread_id = Uuid::new_v4().to_string();
            let message_id = Uuid::new_v4().to_string();
            let message = ReviewMessage {
                id: message_id,
                author: author.clone(),
                body,
                created_at: now,
                status: None,
                activity: None,
                permission: None,
                choice: None,
                extra: serde_json::Map::new(),
            };
            // Every new thread's target is tracked from here on, whoever makes it.
            let location =
                snapshots::track_new(&self.workspace_dir(&review.workspace_root), location);
            let thread = ReviewThread {
                id: thread_id.clone(),
                number: review.next_thread_number,
                location,
                messages: vec![message.clone()],
                status: options.status.unwrap_or(ThreadStatus::Open),
                intent: options.intent.unwrap_or(ThreadIntent::Comment),
                severity: options.severity,
                origin: options.origin.clone(),
                source_id: options.source_id.clone(),
                labels: options.labels.clone(),
                proposal: options.proposal.clone(),
                agent_state: None,
                created_at: now,
                updated_at: now,
                extra: serde_json::Map::new(),
            };
            review.next_thread_number += 1;
            // Check agent queue status before pushing (avoids borrow conflict).
            let should_queue = should_queue_for_agent(review, &thread);
            let mut thread = thread;
            if should_queue {
                thread.agent_state = Some(AgentState::Queued);
            }
            let message_clone = message.clone();
            review.threads.push(thread.clone());
            record(review, ActivityKind::ThreadCreated, &author, Some(&thread_id), now);
            created_thread = Some(thread.clone());
            message_event = Some(MessageAdded {
                review: review.clone(),
                thread,
                message: message_clone,
            });
        })?;

        if let Some(event) = message_event {
            self.fire_message(event);
        }

        Ok((review, created_thread.expect("set during mutate")))
    }

    /// Add a message to a thread.
    pub fn add_message(
        &self,
        review_id: &str,
        thread_id: &str,
        body: String,
        author: Participant,
        choice: Option<String>,
    ) -> Result<ReviewThread, ReviewStoreError> {
        let mut message_event: Option<MessageAdded> = None;
        let thread = self.mutate_thread(review_id, thread_id, |review, thread, now| {
            let message = ReviewMessage {
                id: Uuid::new_v4().to_string(),
                author: author.clone(),
                body,
                created_at: now,
                status: None,
                activity: None,
                permission: None,
                choice,
                extra: serde_json::Map::new(),
            };
            thread.messages.push(message.clone());
            queue_for_agent(review, thread);
            record(review, ActivityKind::MessageAdded, &author, Some(thread_id), now);
            message_event = Some(MessageAdded {
                review: review.clone(),
                thread: thread.clone(),
                message,
            });
        })?;
        if let Some(event) = message_event {
            self.fire_message(event);
        }
        Ok(thread)
    }

    /// Update thread locations from anchor tracking (no activity entry).
    pub fn retrack(
        &self,
        review_id: &str,
        locations: &HashMap<String, CodeLocation>,
    ) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, _now| {
            for thread in &mut review.threads {
                if let Some(loc) = locations.get(&thread.id) {
                    thread.location = loc.clone();
                }
            }
        })
    }

    /// Start a message that will be filled in later. Returns its id.
    pub fn start_message(
        &self,
        review_id: &str,
        thread_id: &str,
        author: Participant,
        init: PartialMessage,
    ) -> Result<String, ReviewStoreError> {
        let message_id = Uuid::new_v4().to_string();
        let id_clone = message_id.clone();
        self.mutate_thread(review_id, thread_id, |review, thread, now| {
            let message = ReviewMessage {
                id: id_clone,
                author: author.clone(),
                body: init.body.unwrap_or_default(),
                created_at: now,
                status: init.status,
                activity: init.activity,
                permission: init.permission,
                choice: init.choice,
                extra: serde_json::Map::new(),
            };
            thread.messages.push(message);
            record(review, ActivityKind::MessageAdded, &author, Some(thread_id), now);
        })?;
        Ok(message_id)
    }

    /// Update fields of an existing message.
    pub fn update_message(
        &self,
        review_id: &str,
        thread_id: &str,
        message_id: &str,
        patch: MessagePatch,
    ) -> Result<ReviewThread, ReviewStoreError> {
        self.mutate_thread(review_id, thread_id, |_review, thread, _now| {
            if let Some(message) = thread.messages.iter_mut().find(|m| m.id == message_id) {
                if let Some(body) = patch.body {
                    message.body = body;
                }
                if let Some(status) = patch.status {
                    message.status = Some(status);
                }
                if let Some(activity) = patch.activity {
                    message.activity = Some(activity);
                }
                if let Some(permission) = patch.permission {
                    message.permission = Some(permission);
                }
                if let Some(choice) = patch.choice {
                    message.choice = Some(choice);
                }
            }
        })
    }

    /// Record a proposal decision and resolve the thread.
    pub fn decide_proposal(
        &self,
        review_id: &str,
        thread_id: &str,
        accepted: bool,
        note: String,
        doc_version: Option<u32>,
    ) -> Result<ReviewThread, ReviewStoreError> {
        self.mutate_thread(review_id, thread_id, |review, thread, now| {
            let Some(proposal) = &mut thread.proposal else {
                return;
            };
            proposal.status = if accepted {
                review_model::ProposalStatus::Accepted
            } else {
                review_model::ProposalStatus::Rejected
            };
            thread.status = ThreadStatus::Resolved;
            let system = Participant {
                id: "system".to_string(),
                kind: review_model::ParticipantKind::System,
                name: "system".to_string(),
            };
            thread.messages.push(ReviewMessage {
                id: Uuid::new_v4().to_string(),
                author: system,
                body: note,
                created_at: now,
                status: None,
                activity: None,
                permission: None,
                choice: None,
                extra: serde_json::Map::new(),
            });
            if let Some(dv) = doc_version {
                if let Some(bundle) = &mut review.bundle {
                    bundle.doc_version = Some(dv);
                }
                // If the doc anchor is text, update it to the accepted replacement
                if let Some(doc_anchor) = &thread.location.doc_anchor
                    && doc_anchor.anchor_type == review_model::DocAnchorType::Text
                    && let Some(after_text) = thread.proposal.as_ref().map(|p| p.after.clone())
                {
                    if let Some(da) = &mut thread.location.doc_anchor {
                        da.exact = Some(after_text.clone());
                    }
                    if let Some(anchor) = &mut thread.location.anchor {
                        anchor.text = after_text;
                    }
                }
            }
        })
    }

    /// Set the agent state on a thread (queued, working, etc.).
    pub fn set_agent_state(
        &self,
        review_id: &str,
        thread_id: &str,
        state: AgentState,
    ) -> Result<ReviewThread, ReviewStoreError> {
        self.mutate_thread(review_id, thread_id, |_review, thread, _now| {
            thread.agent_state = Some(state);
        })
    }

    /// Set the intent on a thread.
    pub fn set_thread_intent(
        &self,
        review_id: &str,
        thread_id: &str,
        intent: ThreadIntent,
    ) -> Result<ReviewThread, ReviewStoreError> {
        self.mutate_thread(review_id, thread_id, |review, thread, _now| {
            thread.intent = intent;
            queue_for_agent(review, thread);
        })
    }

    /// Set the bundle on a review.
    pub fn set_bundle(
        &self,
        review_id: &str,
        bundle: Option<ReviewBundle>,
    ) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, _now| {
            review.bundle = bundle;
        })
    }

    /// Submit the review with a decision.
    pub fn submit(
        &self,
        review_id: &str,
        decision: ReviewDecision,
        summary: String,
        actor: Participant,
    ) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, now| {
            let to_agent = if review
                .agent
                .as_ref()
                .is_some_and(|a| a.transport == Some(AgentTransport::Mcp))
            {
                Some(VerdictToAgent::Waiting)
            } else {
                None
            };
            review.verdict = Some(ReviewVerdict {
                decision,
                summary,
                submitted_at: now,
                count: review.verdict.as_ref().map_or(1, |v| v.count + 1),
                to_agent,
            });
            record(review, ActivityKind::ReviewSubmitted, &actor, None, now);
        })
    }

    /// Set the overview text.
    pub fn set_overview(
        &self,
        review_id: &str,
        overview: String,
    ) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, _now| {
            review.overview = Some(overview);
        })
    }

    /// Mark the review's window as closed or reopened.
    pub fn set_closed(
        &self,
        review_id: &str,
        closed: Option<review_model::ReviewClosed>,
    ) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, _now| {
            review.closed = closed;
        })
    }

    /// Mark that an agent picked up the latest verdict.
    pub fn verdict_delivered(
        &self,
        review_id: &str,
    ) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, _now| {
            if let Some(verdict) = &mut review.verdict {
                verdict.to_agent = Some(VerdictToAgent::Delivered);
            }
        })
    }

    /// Mark files as viewed or not viewed.
    pub fn set_viewed(
        &self,
        review_id: &str,
        paths: &[String],
        viewed: bool,
    ) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, now| {
            let map = review.viewed.get_or_insert_with(HashMap::new);
            for p in paths {
                if viewed {
                    map.insert(p.clone(), now);
                } else {
                    map.remove(p);
                }
            }
        })
    }

    /// Set the connected agent.
    pub fn set_agent(
        &self,
        review_id: &str,
        agent: Option<AgentConfig>,
    ) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, _now| {
            review.agent = agent;
        })
    }

    /// Set the status on a thread (open/resolved/proposed).
    pub fn set_thread_status(
        &self,
        review_id: &str,
        thread_id: &str,
        status: ThreadStatus,
        actor: Participant,
    ) -> Result<ReviewThread, ReviewStoreError> {
        self.mutate_thread(review_id, thread_id, |review, thread, now| {
            if thread.status != status {
                thread.status = status;
                let kind = if status == ThreadStatus::Resolved {
                    ActivityKind::ThreadResolved
                } else {
                    ActivityKind::ThreadReopened
                };
                record(review, kind, &actor, Some(thread_id), now);
            }
        })
    }

    /// Relocate a thread (automatic bookkeeping, no activity entry).
    pub fn relocate_thread(
        &self,
        review_id: &str,
        thread_id: &str,
        location: CodeLocation,
    ) -> Result<ReviewThread, ReviewStoreError> {
        self.mutate_thread(review_id, thread_id, |_review, thread, _now| {
            thread.location = location;
        })
    }

    // ----- mutation infrastructure -----

    /// Mutate a review atomically: read, apply `f`, write, broadcast.
    fn mutate(
        &self,
        review_id: &str,
        f: impl FnOnce(&mut Review, u64),
    ) -> Result<Review, ReviewStoreError> {
        let lock = self.get_review_lock(review_id);
        let _guard = lock.lock();

        let file = self
            .locate(review_id)
            .ok_or_else(|| ReviewStoreError::UnknownReview(review_id.to_string()))?;
        let text = fs::read_to_string(&file)?;
        let mut review: Review = serde_json::from_str(&text)?;

        let now = now_millis();
        f(&mut review, now);
        review.updated_at = now;

        atomic_write(&file, &review)?;
        self.fire_change(ReviewChange::Changed {
            review: review.clone(),
        });
        Ok(review)
    }

    /// Mutate a specific thread within a review.
    fn mutate_thread(
        &self,
        review_id: &str,
        thread_id: &str,
        f: impl FnOnce(&mut Review, &mut ReviewThread, u64),
    ) -> Result<ReviewThread, ReviewStoreError> {
        let lock = self.get_review_lock(review_id);
        let _guard = lock.lock();

        let file = self
            .locate(review_id)
            .ok_or_else(|| ReviewStoreError::UnknownReview(review_id.to_string()))?;
        let text = fs::read_to_string(&file)?;
        let mut review: Review = serde_json::from_str(&text)?;

        let now = now_millis();
        let thread_index = review
            .threads
            .iter()
            .position(|t| t.id == thread_id)
            .ok_or_else(|| ReviewStoreError::UnknownThread {
                review_id: review_id.to_string(),
                thread_id: thread_id.to_string(),
            })?;
        // Remove the thread temporarily so we can pass &mut Review and &mut ReviewThread
        // without a double borrow (the thread is part of review.threads).
        let mut thread = review.threads.remove(thread_index);
        f(&mut review, &mut thread, now);
        thread.updated_at = now;
        review.threads.insert(thread_index, thread.clone());
        review.updated_at = now;
        atomic_write(&file, &review)?;
        self.fire_change(ReviewChange::Changed {
            review: review.clone(),
        });
        Ok(thread)
    }

    fn write_workspace_info(&self, workspace_root: &str) -> Result<(), ReviewStoreError> {
        let root = real_uri(workspace_root);
        let path = file_uri_to_path(&root).unwrap_or_default();
        let info = serde_json::json!({
            "root": root,
            "path": path,
        });
        let ws_dir = self.workspace_dir(workspace_root);
        fs::create_dir_all(&ws_dir)?;
        let file = ws_dir.join("workspace.json");
        let text = serde_json::to_string_pretty(&info)?;
        fs::write(&file, text)?;
        Ok(())
    }
}

impl Default for ReviewStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Atomic write: write to a temp file, then rename.
fn atomic_write(path: &Path, review: &Review) -> Result<(), ReviewStoreError> {
    let text = serde_json::to_string_pretty(review)?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&tmp, &text)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn record(
    review: &mut Review,
    kind: ActivityKind,
    actor: &Participant,
    thread_id: Option<&str>,
    at: u64,
) {
    review.activity.push(ReviewActivity {
        kind,
        actor: actor.clone(),
        thread_id: thread_id.map(|s| s.to_string()),
        at,
    });
    if !review.participants.iter().any(|p| p.id == actor.id) {
        review.participants.push(actor.clone());
    }
}

/// Check whether a thread should be queued for the review's MCP agent.
fn should_queue_for_agent(review: &Review, thread: &ReviewThread) -> bool {
    if let Some(agent) = &review.agent {
        agent.transport == Some(AgentTransport::Mcp)
            && review_model::waits_for_agent(review, thread, &agent.id)
    } else {
        false
    }
}

/// If the review has an MCP agent and the thread should be queued, mark it.
fn queue_for_agent(review: &Review, thread: &mut ReviewThread) {
    if should_queue_for_agent(review, thread) {
        thread.agent_state = Some(AgentState::Queued);
    }
}

// ---------------------------------------------------------------------------
// Partial types for message operations
// ---------------------------------------------------------------------------

/// Fields for initializing a started message.
#[derive(Default)]
pub struct PartialMessage {
    pub body: Option<String>,
    pub status: Option<review_model::MessageStatus>,
    pub activity: Option<Vec<review_model::AgentActivity>>,
    pub permission: Option<review_model::PermissionRequest>,
    pub choice: Option<String>,
}

/// Fields to patch on an existing message.
#[derive(Default)]
pub struct MessagePatch {
    pub body: Option<String>,
    pub status: Option<review_model::MessageStatus>,
    pub activity: Option<Vec<review_model::AgentActivity>>,
    pub permission: Option<review_model::PermissionRequest>,
    pub choice: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use review_model::{LocationKind, ParticipantKind, ReviewScope};
    use std::sync::Arc;
    use std::thread;

    fn test_author() -> Participant {
        Participant {
            id: "human:test@test.com".into(),
            kind: ParticipantKind::Human,
            name: "Test User".into(),
        }
    }

    fn test_location() -> CodeLocation {
        CodeLocation {
            kind: LocationKind::Repository,
            uri: None,
            range: None,
            symbol: None,
            anchor: None,
            doc_anchor: None,
            patch_anchor: None,
            tracked: None,
            extra: serde_json::Map::new(),
        }
    }

    fn setup_store(tmp: &tempfile::TempDir) -> Arc<ReviewStore> {
        Arc::new(ReviewStore::with_home(tmp.path().to_path_buf()))
    }

    #[test]
    fn test_create_and_get() {
        let tmp = tempfile::tempdir().unwrap();
        let store = setup_store(&tmp);
        let review = store
            .create(CreateReviewParams {
                workspace_root: "file:///tmp/test".into(),
                title: "Test Review".into(),
                scope: ReviewScope::Repository,
                author: test_author(),
            })
            .unwrap();
        assert_eq!(review.title, "Test Review");
        assert_eq!(review.threads.len(), 0);
        assert_eq!(review.next_thread_number, 1);

        let got = store.get(&review.id).unwrap();
        assert_eq!(got.id, review.id);
        assert_eq!(got.title, "Test Review");
    }

    #[test]
    fn test_list() {
        let tmp = tempfile::tempdir().unwrap();
        let store = setup_store(&tmp);
        store
            .create(CreateReviewParams {
                workspace_root: "file:///tmp/test".into(),
                title: "First".into(),
                scope: ReviewScope::Repository,
                author: test_author(),
            })
            .unwrap();
        store
            .create(CreateReviewParams {
                workspace_root: "file:///tmp/test".into(),
                title: "Second".into(),
                scope: ReviewScope::Repository,
                author: test_author(),
            })
            .unwrap();
        let reviews = store.list("file:///tmp/test");
        assert_eq!(reviews.len(), 2);
    }

    #[test]
    fn test_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let store = setup_store(&tmp);
        let review = store
            .create(CreateReviewParams {
                workspace_root: "file:///tmp/test".into(),
                title: "To Delete".into(),
                scope: ReviewScope::Repository,
                author: test_author(),
            })
            .unwrap();
        store.delete(&review.id).unwrap();
        assert!(store.get(&review.id).is_none());
    }

    #[test]
    fn test_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let store = setup_store(&tmp);
        let review = store
            .create(CreateReviewParams {
                workspace_root: "file:///tmp/test".into(),
                title: "Original".into(),
                scope: ReviewScope::Repository,
                author: test_author(),
            })
            .unwrap();
        let renamed = store.rename(&review.id, "Renamed".into()).unwrap();
        assert_eq!(renamed.title, "Renamed");
    }

    #[test]
    fn test_archive_unarchive() {
        let tmp = tempfile::tempdir().unwrap();
        let store = setup_store(&tmp);
        let review = store
            .create(CreateReviewParams {
                workspace_root: "file:///tmp/test".into(),
                title: "Archivable".into(),
                scope: ReviewScope::Repository,
                author: test_author(),
            })
            .unwrap();
        let archived = store.archive(&review.id, true).unwrap();
        assert!(archived.archived_at.is_some());
        let unarchived = store.archive(&review.id, false).unwrap();
        assert!(unarchived.archived_at.is_none());
    }

    #[test]
    fn test_create_thread_and_message() {
        let tmp = tempfile::tempdir().unwrap();
        let store = setup_store(&tmp);
        let review = store
            .create(CreateReviewParams {
                workspace_root: "file:///tmp/test".into(),
                title: "Threads".into(),
                scope: ReviewScope::Repository,
                author: test_author(),
            })
            .unwrap();
        let (updated, thread) = store
            .create_thread(
                &review.id,
                test_location(),
                "First comment".into(),
                test_author(),
                Default::default(),
            )
            .unwrap();
        assert_eq!(thread.number, 1);
        assert_eq!(thread.messages.len(), 1);
        assert_eq!(thread.messages[0].body, "First comment");
        assert_eq!(updated.threads.len(), 1);

        let thread2 = store
            .add_message(
                &review.id,
                &thread.id,
                "Reply".into(),
                test_author(),
                None,
            )
            .unwrap();
        assert_eq!(thread2.messages.len(), 2);
        assert_eq!(thread2.messages[1].body, "Reply");
    }

    #[test]
    fn test_create_thread_tracks_its_target() {
        let tmp = tempfile::tempdir().unwrap();
        let store = setup_store(&tmp);
        let repository = tmp.path().join("repository");
        fs::create_dir_all(&repository).unwrap();
        let file = repository.join("main.rs");
        fs::write(&file, "fn main() {\n    run();\n}\n").unwrap();
        let workspace_root = path_to_file_uri(&repository.to_string_lossy());
        let review = store
            .create(CreateReviewParams {
                workspace_root: workspace_root.clone(),
                title: "Tracked".into(),
                scope: ReviewScope::Repository,
                author: test_author(),
            })
            .unwrap();
        let position = |line, character| review_model::Position { line, character };
        let location = CodeLocation {
            kind: LocationKind::Line,
            uri: Some(path_to_file_uri(&file.to_string_lossy())),
            range: Some(review_model::Range {
                start: position(1, 4),
                end: position(1, 10),
            }),
            anchor: Some(review_model::CodeAnchor {
                text: "run();".into(),
                before: None,
                after: None,
                syntax: None,
            }),
            ..test_location()
        };
        let (_, thread) = store
            .create_thread(&review.id, location, "Why?".into(), test_author(), Default::default())
            .unwrap();
        let tracked = thread.location.tracked.expect("a new line thread is tracked");
        assert_eq!((tracked.from, tracked.to), (16, 22));
        assert_eq!(tracked.text, "run();");
        let snapshot = snapshots::snapshots_dir(&store.workspace_dir(&workspace_root))
            .join(&tracked.version);
        assert_eq!(
            fs::read_to_string(snapshot).unwrap(),
            "fn main() {\n    run();\n}\n"
        );
    }

    #[test]
    fn test_thread_status() {
        let tmp = tempfile::tempdir().unwrap();
        let store = setup_store(&tmp);
        let review = store
            .create(CreateReviewParams {
                workspace_root: "file:///tmp/test".into(),
                title: "Status".into(),
                scope: ReviewScope::Repository,
                author: test_author(),
            })
            .unwrap();
        let (_, thread) = store
            .create_thread(
                &review.id,
                test_location(),
                "Comment".into(),
                test_author(),
                Default::default(),
            )
            .unwrap();
        let resolved = store
            .set_thread_status(&review.id, &thread.id, ThreadStatus::Resolved, test_author())
            .unwrap();
        assert_eq!(resolved.status, ThreadStatus::Resolved);
    }

    #[test]
    fn test_submit_verdict() {
        let tmp = tempfile::tempdir().unwrap();
        let store = setup_store(&tmp);
        let review = store
            .create(CreateReviewParams {
                workspace_root: "file:///tmp/test".into(),
                title: "Submit".into(),
                scope: ReviewScope::Repository,
                author: test_author(),
            })
            .unwrap();
        let submitted = store
            .submit(
                &review.id,
                ReviewDecision::Approve,
                "Looks good".into(),
                test_author(),
            )
            .unwrap();
        assert_eq!(
            submitted.verdict.as_ref().unwrap().decision,
            ReviewDecision::Approve
        );
        assert_eq!(submitted.verdict.as_ref().unwrap().count, 1);
    }

    #[test]
    fn test_set_viewed() {
        let tmp = tempfile::tempdir().unwrap();
        let store = setup_store(&tmp);
        let review = store
            .create(CreateReviewParams {
                workspace_root: "file:///tmp/test".into(),
                title: "Viewed".into(),
                scope: ReviewScope::Repository,
                author: test_author(),
            })
            .unwrap();
        let updated = store
            .set_viewed(&review.id, &["src/main.rs".into()], true)
            .unwrap();
        assert!(updated.viewed.as_ref().unwrap().contains_key("src/main.rs"));
        let cleared = store
            .set_viewed(&review.id, &["src/main.rs".into()], false)
            .unwrap();
        assert!(!cleared
            .viewed
            .as_ref()
            .unwrap()
            .contains_key("src/main.rs"));
    }

    #[test]
    fn test_change_subscription() {
        let tmp = tempfile::tempdir().unwrap();
        let store = setup_store(&tmp);
        let rx = store.subscribe_changes();

        let review = store
            .create(CreateReviewParams {
                workspace_root: "file:///tmp/test".into(),
                title: "Events".into(),
                scope: ReviewScope::Repository,
                author: test_author(),
            })
            .unwrap();

        let event = rx.try_recv().unwrap();
        match event {
            ReviewChange::Changed { review: r } => assert_eq!(r.id, review.id),
            ReviewChange::Deleted { .. } => panic!("expected Changed"),
        }
    }

    #[test]
    fn test_workspace_dir_hash() {
        use sha2::Digest;
        let tmp = tempfile::tempdir().unwrap();
        let store = setup_store(&tmp);
        // Use a URI that won't change under real_path resolution
        // (the path doesn't exist, so its parent resolves but basename is kept).
        // Instead of hardcoding, verify the hash matches sha256 of the resolved URI.
        let input = "file:///nonexistent_test_path_12345/project";
        let dir = store.workspace_dir(input);
        let resolved = crate::real_uri(input);
        let hash = format!("{:x}", sha2::Sha256::digest(resolved.as_bytes()));
        let expected_suffix = &hash[..16];
        assert!(
            dir.ends_with(expected_suffix),
            "hash mismatch: dir={dir:?}, expected suffix={expected_suffix}"
        );
    }

    #[test]
    fn test_json_round_trip_with_ts() {
        let tmp = tempfile::tempdir().unwrap();
        let store = setup_store(&tmp);
        let review = store
            .create(CreateReviewParams {
                workspace_root: "file:///tmp/test".into(),
                title: "Round Trip".into(),
                scope: ReviewScope::Repository,
                author: test_author(),
            })
            .unwrap();

        // Read back the raw JSON and verify it matches our model
        let got = store.get(&review.id).unwrap();
        assert_eq!(got.id, review.id);

        // Verify the JSON is valid and has camelCase keys
        let file = store.locate(&review.id).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert!(raw.get("workspaceRoot").is_some());
        assert!(raw.get("createdAt").is_some());
        assert!(raw.get("updatedAt").is_some());
        assert!(raw.get("nextThreadNumber").is_some());
    }

    /// Stress test: many threads mutating the same review concurrently.
    #[test]
    fn test_concurrent_mutations() {
        let tmp = tempfile::tempdir().unwrap();
        let store = setup_store(&tmp);
        let review = store
            .create(CreateReviewParams {
                workspace_root: "file:///tmp/test".into(),
                title: "Concurrent".into(),
                scope: ReviewScope::Repository,
                author: test_author(),
            })
            .unwrap();
        let review_id = review.id.clone();

        let num_threads = 10;
        let ops_per_thread = 5;
        let store = Arc::new(store);

        let handles: Vec<_> = (0..num_threads)
            .map(|i| {
                let store = store.clone();
                let review_id = review_id.clone();
                thread::spawn(move || {
                    for j in 0..ops_per_thread {
                        let author = Participant {
                            id: format!("human:thread{i}"),
                            kind: ParticipantKind::Human,
                            name: format!("Thread {i}"),
                        };
                        store
                            .create_thread(
                                &review_id,
                                test_location(),
                                format!("Thread {i}, comment {j}"),
                                author,
                                Default::default(),
                            )
                            .unwrap();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let final_review = store.get(&review_id).unwrap();
        assert_eq!(
            final_review.threads.len(),
            num_threads * ops_per_thread,
            "all threads should have been created"
        );

        // Verify thread numbers are unique and sequential
        let mut numbers: Vec<u32> = final_review.threads.iter().map(|t| t.number).collect();
        numbers.sort();
        let expected: Vec<u32> = (1..=(num_threads * ops_per_thread) as u32).collect();
        assert_eq!(numbers, expected, "thread numbers should be sequential");

        // Verify the file is valid JSON
        let file = store.locate(&review_id).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        let _: Review = serde_json::from_str(&text).unwrap();
    }

    /// Stress test: concurrent mutations on the same thread (add messages).
    #[test]
    fn test_concurrent_messages() {
        let tmp = tempfile::tempdir().unwrap();
        let store = setup_store(&tmp);
        let review = store
            .create(CreateReviewParams {
                workspace_root: "file:///tmp/test".into(),
                title: "Messages".into(),
                scope: ReviewScope::Repository,
                author: test_author(),
            })
            .unwrap();
        let (_, thread) = store
            .create_thread(
                &review.id,
                test_location(),
                "Initial".into(),
                test_author(),
                Default::default(),
            )
            .unwrap();

        let num_writers = 8;
        let msgs_per_writer = 5;
        let store = Arc::new(store);

        let handles: Vec<_> = (0..num_writers)
            .map(|i| {
                let store = store.clone();
                let review_id = review.id.clone();
                let thread_id = thread.id.clone();
                thread::spawn(move || {
                    for j in 0..msgs_per_writer {
                        store
                            .add_message(
                                &review_id,
                                &thread_id,
                                format!("Writer {i} msg {j}"),
                                Participant {
                                    id: format!("human:writer{i}"),
                                    kind: ParticipantKind::Human,
                                    name: format!("Writer {i}"),
                                },
                                None,
                            )
                            .unwrap();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let final_review = store.get(&review.id).unwrap();
        let thread = &final_review.threads[0];
        // 1 initial + num_writers * msgs_per_writer
        assert_eq!(
            thread.messages.len(),
            1 + num_writers * msgs_per_writer,
            "all messages should be preserved"
        );
    }
}
