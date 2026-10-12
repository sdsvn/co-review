//! Persistent review storage under `~/.co-review`, compatible with the TypeScript app.
//!
//! Reviews are stored as JSON files:
//! `$CO_REVIEW_HOME/workspaces/<sha256(root_uri)[0:16]>/reviews/<id>.json`
//!
//! All mutations are serialized per review (internal locking) and broadcast to subscribers,
//! so every frontend and agent sees the same state.

mod file_uri;
mod participants;
mod store;

pub use file_uri::{path_to_file_uri, real_path, real_uri};
pub use participants::current_user;
pub use store::{
    CreateReviewParams, MessageAdded, ReviewChange, ReviewStore, ReviewStoreError,
};

/// Where Co-Review keeps its state: `$CO_REVIEW_HOME`, default `~/.co-review`.
pub fn co_review_home() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("CO_REVIEW_HOME") {
        return std::path::PathBuf::from(home);
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(profile) = std::env::var("USERPROFILE") {
            return std::path::PathBuf::from(profile).join(".co-review");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return std::path::PathBuf::from(home).join(".co-review");
    }
    // Last resort fallback
    std::path::PathBuf::from(".co-review")
}
