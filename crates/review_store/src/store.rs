//! The core review store: persistence, mutations, locking and change events.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use review_model::{
    ActivityKind, AgentConfig, AgentState, AgentTransport, CodeLocation, DocAnchorType,
    Participant, ParticipantKind, ProposalStatus, Review, ReviewActivity, ReviewBundle,
    ReviewClosed, ReviewDecision, ReviewMessage, ReviewScope, ReviewThread, ReviewVerdict,
    ThreadIntent, ThreadOptions, ThreadStatus, VerdictToAgent,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::file_uri::{file_uri_to_path, path_to_file_uri, real_uri};
use crate::snapshots;

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

/// Broadcast when a review changes or is deleted.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum ReviewChange {
    Changed { review: Review },
    Deleted { review_id: String, workspace_root: String },
}

pub struct CreateReviewParams {
    pub workspace_root: String,
    pub title: String,
    pub scope: ReviewScope,
    pub author: Participant,
}

/// Fields of a message to set: all of them when starting one, the given ones when updating.
#[derive(Default)]
pub struct MessagePatch {
    pub body: Option<String>,
    pub status: Option<review_model::MessageStatus>,
    pub activity: Option<Vec<review_model::AgentActivity>>,
    pub permission: Option<review_model::PermissionRequest>,
    pub choice: Option<String>,
}

#[derive(Default)]
struct StoreInner {
    /// reviewId -> file path, populated lazily.
    locations: HashMap<String, PathBuf>,
    /// Held during a review's whole read-modify-write-broadcast cycle.
    locks: HashMap<String, Arc<Mutex<()>>>,
    change_senders: Vec<smol::channel::Sender<ReviewChange>>,
}

/// Local, file-based review persistence, outside the repository:
/// `<home>/workspaces/<hash of root URI>/reviews/<id>.json`.
///
/// Mutations are serialized per review, from std threads or smol tasks alike.
pub struct ReviewStore {
    home: PathBuf,
    inner: Mutex<StoreInner>,
}

impl ReviewStore {
    /// A store under `home`; Co-Review's own is `crate::co_review_home()`.
    pub fn with_home(home: PathBuf) -> Self {
        Self {
            home,
            inner: Mutex::default(),
        }
    }

    /// Where the reviews (and other state) of a workspace live, outside the repository.
    pub fn workspace_dir(&self, workspace_root: &str) -> PathBuf {
        let hash = format!("{:x}", Sha256::digest(real_uri(workspace_root).as_bytes()));
        self.home.join("workspaces").join(&hash[..16])
    }

    fn reviews_dir(&self, workspace_root: &str) -> PathBuf {
        self.workspace_dir(workspace_root).join("reviews")
    }

    pub fn subscribe_changes(&self) -> smol::channel::Receiver<ReviewChange> {
        let (sender, receiver) = smol::channel::unbounded();
        self.inner.lock().change_senders.push(sender);
        receiver
    }

    fn fire_change(&self, change: ReviewChange) {
        self.inner
            .lock()
            .change_senders
            .retain(|s| s.try_send(change.clone()).is_ok());
    }

    fn remember_location(&self, review_id: &str, path: PathBuf) {
        self.inner
            .lock()
            .locations
            .insert(review_id.to_string(), path);
    }

    /// The file of a review, searching all workspaces if not already known.
    fn locate(&self, review_id: &str) -> Option<PathBuf> {
        if let Some(known) = self.inner.lock().locations.get(review_id) {
            return Some(known.clone());
        }
        let entries = fs::read_dir(self.home.join("workspaces")).ok()?;
        let file = entries
            .flatten()
            .map(|entry| entry.path().join("reviews").join(format!("{review_id}.json")))
            .find(|file| file.exists())?;
        self.remember_location(review_id, file.clone());
        Some(file)
    }

    /// The reviews of a workspace, most recently updated first.
    pub fn list(&self, workspace_root: &str) -> Vec<Review> {
        let Ok(entries) = fs::read_dir(self.reviews_dir(workspace_root)) else {
            return Vec::new();
        };
        let mut reviews: Vec<Review> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|e| e == "json"))
            .filter_map(|path| {
                let review = read(&path).ok()?;
                self.remember_location(&review.id, path);
                Some(review)
            })
            .collect();
        reviews.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
        reviews
    }

    /// The reviews a window should show: the workspace's own, plus change reviews
    /// whose code folder is this workspace.
    pub fn list_for_window(&self, workspace_root: &str) -> Vec<Review> {
        let root = real_uri(workspace_root);
        let mut all = self.list(&root);
        let own: HashSet<String> = all.iter().map(|r| r.id.clone()).collect();
        all.extend(self.list_all().into_iter().filter(|r| {
            !own.contains(&r.id)
                && r.bundle
                    .as_ref()
                    .and_then(|bundle| bundle.code.as_deref())
                    .is_some_and(|code| path_to_file_uri(code) == root)
        }));
        all.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
        all
    }

    /// Every review in every workspace.
    fn list_all(&self) -> Vec<Review> {
        let Ok(entries) = fs::read_dir(self.home.join("workspaces")) else {
            return Vec::new();
        };
        let mut all: Vec<Review> = entries
            .flatten()
            .filter_map(|entry| {
                let text = fs::read_to_string(entry.path().join("workspace.json")).ok()?;
                let info: serde_json::Value = serde_json::from_str(&text).ok()?;
                Some(self.list(info.get("root")?.as_str()?))
            })
            .flatten()
            .collect();
        all.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
        all
    }

    pub fn get(&self, review_id: &str) -> Option<Review> {
        read(&self.locate(review_id)?).ok()
    }

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
        let workspace_dir = self.workspace_dir(&workspace_root);
        let dir = workspace_dir.join("reviews");
        fs::create_dir_all(&dir)?;
        let info = serde_json::json!({
            "root": workspace_root,
            "path": file_uri_to_path(&workspace_root).unwrap_or_default(),
        });
        fs::write(
            workspace_dir.join("workspace.json"),
            serde_json::to_string_pretty(&info)?,
        )?;
        let file = dir.join(format!("{}.json", review.id));
        self.remember_location(&review.id, file.clone());
        atomic_write(&file, &review)?;
        self.fire_change(ReviewChange::Changed {
            review: review.clone(),
        });
        Ok(review)
    }

    pub fn delete(&self, review_id: &str) -> Result<(), ReviewStoreError> {
        let unknown = || ReviewStoreError::UnknownReview(review_id.to_string());
        let file = self.locate(review_id).ok_or_else(unknown)?;
        let review = read(&file).map_err(|_| unknown())?;
        let _ = fs::remove_file(&file);
        self.inner.lock().locations.remove(review_id);
        self.fire_change(ReviewChange::Deleted {
            review_id: review_id.to_string(),
            workspace_root: review.workspace_root,
        });
        Ok(())
    }

    pub fn rename(&self, review_id: &str, title: String) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, _| review.title = title)
    }

    pub fn archive(&self, review_id: &str, archived: bool) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, now| {
            review.archived_at = archived.then_some(now);
        })
    }

    /// Returns the updated review and the new thread.
    pub fn create_thread(
        &self,
        review_id: &str,
        location: CodeLocation,
        body: String,
        author: Participant,
        options: ThreadOptions,
    ) -> Result<(Review, ReviewThread), ReviewStoreError> {
        self.update(review_id, |review, now| {
            let message = new_message(author.clone(), body, now, MessagePatch::default());
            // Every new thread's target is tracked from here on, whoever makes it.
            let location =
                snapshots::track_new(&self.workspace_dir(&review.workspace_root), location);
            let mut thread = ReviewThread {
                id: Uuid::new_v4().to_string(),
                number: review.next_thread_number,
                location,
                messages: vec![message],
                status: options.status.unwrap_or(ThreadStatus::Open),
                intent: options.intent.unwrap_or(ThreadIntent::Comment),
                severity: options.severity,
                origin: options.origin,
                source_id: options.source_id,
                labels: options.labels,
                proposal: options.proposal,
                agent_state: None,
                created_at: now,
                updated_at: now,
                extra: serde_json::Map::new(),
            };
            review.next_thread_number += 1;
            queue_for_agent(review, &mut thread);
            review.threads.push(thread.clone());
            record(review, ActivityKind::ThreadCreated, &author, Some(&thread.id), now);
            Ok(thread)
        })
    }

    pub fn add_message(
        &self,
        review_id: &str,
        thread_id: &str,
        body: String,
        author: Participant,
        choice: Option<String>,
    ) -> Result<ReviewThread, ReviewStoreError> {
        self.mutate_thread(review_id, thread_id, |review, thread, now| {
            let init = MessagePatch {
                choice,
                ..MessagePatch::default()
            };
            thread.messages.push(new_message(author.clone(), body, now, init));
            queue_for_agent(review, thread);
            record(review, ActivityKind::MessageAdded, &author, Some(thread_id), now);
        })
    }

    /// Update thread locations from anchor tracking (no activity entry).
    pub fn retrack(
        &self,
        review_id: &str,
        locations: &HashMap<String, CodeLocation>,
    ) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, _| {
            for thread in &mut review.threads {
                if let Some(location) = locations.get(&thread.id) {
                    thread.location = location.clone();
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
        mut init: MessagePatch,
    ) -> Result<String, ReviewStoreError> {
        let mut message_id = String::new();
        self.mutate_thread(review_id, thread_id, |review, thread, now| {
            let body = init.body.take().unwrap_or_default();
            let message = new_message(author.clone(), body, now, init);
            message_id = message.id.clone();
            thread.messages.push(message);
            record(review, ActivityKind::MessageAdded, &author, Some(thread_id), now);
        })?;
        Ok(message_id)
    }

    pub fn update_message(
        &self,
        review_id: &str,
        thread_id: &str,
        message_id: &str,
        patch: MessagePatch,
    ) -> Result<ReviewThread, ReviewStoreError> {
        self.mutate_thread(review_id, thread_id, |_, thread, _| {
            let Some(message) = thread.messages.iter_mut().find(|m| m.id == message_id) else {
                return;
            };
            if let Some(body) = patch.body {
                message.body = body;
            }
            message.status = patch.status.or(message.status);
            message.activity = patch.activity.or(message.activity.take());
            message.permission = patch.permission.or(message.permission.take());
            message.choice = patch.choice.or(message.choice.take());
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
                ProposalStatus::Accepted
            } else {
                ProposalStatus::Rejected
            };
            thread.status = ThreadStatus::Resolved;
            let system = Participant {
                id: "system".to_string(),
                kind: ParticipantKind::System,
                name: "system".to_string(),
            };
            thread
                .messages
                .push(new_message(system, note, now, MessagePatch::default()));
            let Some(doc_version) = doc_version else {
                return;
            };
            if let Some(bundle) = &mut review.bundle {
                bundle.doc_version = Some(doc_version);
            }
            // A text anchor now quotes the accepted replacement.
            if let Some(doc_anchor) = &mut thread.location.doc_anchor
                && doc_anchor.anchor_type == DocAnchorType::Text
            {
                doc_anchor.exact = Some(proposal.after.clone());
                if let Some(anchor) = &mut thread.location.anchor {
                    anchor.text = proposal.after.clone();
                }
            }
        })
    }

    pub fn set_agent_state(
        &self,
        review_id: &str,
        thread_id: &str,
        state: AgentState,
    ) -> Result<ReviewThread, ReviewStoreError> {
        self.mutate_thread(review_id, thread_id, |_, thread, _| {
            thread.agent_state = Some(state);
        })
    }

    pub fn set_thread_intent(
        &self,
        review_id: &str,
        thread_id: &str,
        intent: ThreadIntent,
    ) -> Result<ReviewThread, ReviewStoreError> {
        self.mutate_thread(review_id, thread_id, |review, thread, _| {
            thread.intent = intent;
            queue_for_agent(review, thread);
        })
    }

    pub fn set_bundle(
        &self,
        review_id: &str,
        bundle: Option<ReviewBundle>,
    ) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, _| review.bundle = bundle)
    }

    pub fn submit(
        &self,
        review_id: &str,
        decision: ReviewDecision,
        summary: String,
        actor: Participant,
    ) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, now| {
            let to_agent = review
                .agent
                .as_ref()
                .is_some_and(|a| a.transport == Some(AgentTransport::Mcp))
                .then_some(VerdictToAgent::Waiting);
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

    pub fn set_overview(&self, review_id: &str, overview: String) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, _| review.overview = Some(overview))
    }

    /// Mark the review's window as closed or reopened.
    pub fn set_closed(
        &self,
        review_id: &str,
        closed: Option<ReviewClosed>,
    ) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, _| review.closed = closed)
    }

    /// Mark that an agent picked up the latest verdict.
    pub fn verdict_delivered(&self, review_id: &str) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, _| {
            if let Some(verdict) = &mut review.verdict {
                verdict.to_agent = Some(VerdictToAgent::Delivered);
            }
        })
    }

    pub fn set_viewed(
        &self,
        review_id: &str,
        paths: &[String],
        viewed: bool,
    ) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, now| {
            let map = review.viewed.get_or_insert_with(HashMap::new);
            for path in paths {
                if viewed {
                    map.insert(path.clone(), now);
                } else {
                    map.remove(path);
                }
            }
        })
    }

    pub fn set_agent(
        &self,
        review_id: &str,
        agent: Option<AgentConfig>,
    ) -> Result<Review, ReviewStoreError> {
        self.mutate(review_id, |review, _| review.agent = agent)
    }

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

    /// Read a review, apply `f`, write it and broadcast it, all under the review's lock.
    fn update<T>(
        &self,
        review_id: &str,
        f: impl FnOnce(&mut Review, u64) -> Result<T, ReviewStoreError>,
    ) -> Result<(Review, T), ReviewStoreError> {
        let lock = Arc::clone(self.inner.lock().locks.entry(review_id.to_string()).or_default());
        let _guard = lock.lock();

        let file = self
            .locate(review_id)
            .ok_or_else(|| ReviewStoreError::UnknownReview(review_id.to_string()))?;
        let mut review = read(&file)?;
        let now = now_millis();
        let value = f(&mut review, now)?;
        review.updated_at = now;
        atomic_write(&file, &review)?;
        self.fire_change(ReviewChange::Changed {
            review: review.clone(),
        });
        Ok((review, value))
    }

    fn mutate(
        &self,
        review_id: &str,
        f: impl FnOnce(&mut Review, u64),
    ) -> Result<Review, ReviewStoreError> {
        self.update(review_id, |review, now| {
            f(review, now);
            Ok(())
        })
        .map(|(review, ())| review)
    }

    fn mutate_thread(
        &self,
        review_id: &str,
        thread_id: &str,
        f: impl FnOnce(&mut Review, &mut ReviewThread, u64),
    ) -> Result<ReviewThread, ReviewStoreError> {
        self.update(review_id, |review, now| {
            let index = review
                .threads
                .iter()
                .position(|t| t.id == thread_id)
                .ok_or_else(|| ReviewStoreError::UnknownThread {
                    review_id: review_id.to_string(),
                    thread_id: thread_id.to_string(),
                })?;
            // Taken out so `f` can borrow the review and the thread mutably at once.
            let mut thread = review.threads.remove(index);
            f(review, &mut thread, now);
            thread.updated_at = now;
            review.threads.insert(index, thread.clone());
            Ok(thread)
        })
        .map(|(_, thread)| thread)
    }
}

fn read(file: &Path) -> Result<Review, ReviewStoreError> {
    Ok(serde_json::from_str(&fs::read_to_string(file)?)?)
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Write to a temporary file, then rename, so a crash never leaves a half-written review.
fn atomic_write(path: &Path, review: &Review) -> Result<(), ReviewStoreError> {
    let text = serde_json::to_string_pretty(review)?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&tmp, &text)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn new_message(author: Participant, body: String, now: u64, init: MessagePatch) -> ReviewMessage {
    ReviewMessage {
        id: Uuid::new_v4().to_string(),
        author,
        body,
        created_at: now,
        status: init.status,
        activity: init.activity,
        permission: init.permission,
        choice: init.choice,
        extra: serde_json::Map::new(),
    }
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
        thread_id: thread_id.map(str::to_string),
        at,
    });
    if !review.participants.iter().any(|p| p.id == actor.id) {
        review.participants.push(actor.clone());
    }
}

/// Queue the thread for the review's MCP agent when its latest message waits for it.
fn queue_for_agent(review: &Review, thread: &mut ReviewThread) {
    if let Some(agent) = &review.agent
        && agent.transport == Some(AgentTransport::Mcp)
        && review_model::waits_for_agent(review, thread, &agent.id)
    {
        thread.agent_state = Some(AgentState::Queued);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use review_model::LocationKind;
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
