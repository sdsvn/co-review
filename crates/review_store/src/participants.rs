//! Participant identification from the repository's git config.

use review_model::{Participant, ParticipantKind};
use std::path::Path;
use std::process::Command;

fn git_config(cwd: &Path, key: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["config", key])
        .current_dir(cwd)
        .output()
        .ok()?;
    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if value.is_empty() {
            None
        } else {
            Some(value)
        }
    } else {
        None
    }
}

/// The human reviewer, from the repository's git config (falls back to the OS user).
pub fn current_user(workspace_path: &Path) -> Participant {
    let email = git_config(workspace_path, "user.email");
    let name = git_config(workspace_path, "user.name").unwrap_or_else(|| {
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "unknown".to_string())
    });
    let id = format!("human:{}", email.as_deref().unwrap_or(&name));
    Participant {
        id,
        kind: ParticipantKind::Human,
        name,
    }
}
