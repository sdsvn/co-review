//! Graphify runner: installed check, one build per root at a time, freshness, worktree seeding.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A graph younger than this is fresh enough for the overview; rebuilding is not worth the wait.
const FRESH_MS: u64 = 60_000;

/// The outcome of a `build_graph` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphState {
    /// The graph was built (or was already fresh).
    Built,
    /// The `graphify` command is not installed.
    Missing,
    /// `graphify update` failed.
    Failed,
}

/// Persisted state of the last successful build, stored in `graphify-out/.co-review-build-state.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BuildState {
    /// The HEAD commit at build time.
    head: String,
    /// SHA-256 of `git status --porcelain` output at build time.
    status_hash: String,
    /// Unix timestamp in milliseconds when the build completed.
    built_at: u64,
}

/// Per-root build locks so only one `graphify update` runs per root at a time.
static BUILD_LOCKS: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether graphify has been found on PATH.
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Whether the `graphify` command is on the PATH.
///
/// Only a `true` result is cached, so installing graphify later needs no restart.
fn graphify_installed() -> bool {
    if INSTALLED.load(Ordering::Relaxed) {
        return true;
    }
    let ok = Command::new("graphify")
        .arg("--help")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        INSTALLED.store(true, Ordering::Relaxed);
    }
    ok
}

/// Build or refresh `<root>/graphify-out/graph.json`.
///
/// Returns `Missing` when `graphify` is not installed. Serializes builds per root so only one
/// `graphify update` runs at a time. Skips the build when HEAD + working-tree status are unchanged
/// or the graph is within the freshness window.
///
/// When the root has no `graphify-out/` yet, looks for a sibling worktree of the same repository
/// that has one and seeds from it (copy-on-write on macOS).
pub fn build_graph(root: &Path) -> GraphState {
    if !graphify_installed() {
        return GraphState::Missing;
    }

    // Acquire per-root lock.
    let lock = {
        let mut locks = BUILD_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(locks.entry(root.to_path_buf()).or_default())
    };
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());

    match build_graph_inner(root) {
        Ok(state) => state,
        Err(err) => {
            log::error!("[co-review] graphify build error for {}: {err:#}", root.display());
            GraphState::Failed
        }
    }
}

fn build_graph_inner(root: &Path) -> Result<GraphState> {
    let out_dir = root.join("graphify-out");
    let graph_file = out_dir.join("graph.json");
    let state_file = out_dir.join(".co-review-build-state.json");
    let existed = out_dir.is_dir();

    let saved_state = std::fs::read_to_string(&state_file)
        .ok()
        .and_then(|text| serde_json::from_str::<BuildState>(&text).ok());

    if let Some(ref state) = saved_state && existed && now_ms().saturating_sub(state.built_at) < FRESH_MS {
        return Ok(GraphState::Built);
    }

    // Check if HEAD + working tree are unchanged.
    let current_head = git_head(root).unwrap_or_default();
    let current_status_hash = git_status_hash(root).unwrap_or_default();

    if let Some(ref state) = saved_state && existed && graph_file.is_file()
        && state.head == current_head && state.status_hash == current_status_hash
    {
        // Nothing changed; update the timestamp so we stay in the freshness window.
        write_build_state(&state_file, current_head, current_status_hash);
        return Ok(GraphState::Built);
    }

    // Seed from a sibling worktree if graphify-out/ doesn't exist here yet.
    if !existed && let Err(err) = seed_from_sibling(root) {
        log::warn!("[co-review] could not seed graphify-out from sibling: {err:#}");
    }

    // Run graphify update.
    let output = Command::new("graphify")
        .args(["update", &root.to_string_lossy()])
        .current_dir(root)
        .output()
        .context("failed to run graphify update")?;

    if !graph_file.is_file() {
        let shown = if output.stdout.is_empty() {
            &output.stderr
        } else {
            &output.stdout
        };
        let shown = String::from_utf8_lossy(shown);
        let mut start = shown.len().saturating_sub(400);
        while !shown.is_char_boundary(start) {
            start += 1;
        }
        let tail = &shown[start..];
        log::error!(
            "[co-review] graphify update {} failed{}",
            root.display(),
            if tail.is_empty() {
                String::new()
            } else {
                format!(": {tail}")
            }
        );
        return Ok(GraphState::Failed);
    }

    write_build_state(&state_file, current_head, current_status_hash);

    // Exclude from git if we created graphify-out/.
    if !existed && let Err(err) = exclude_from_git(root) {
        log::warn!("[co-review] could not add graphify-out to git exclude: {err:#}");
    }

    Ok(GraphState::Built)
}

/// Remembers a build, best effort: without it the next one just runs `graphify update` again.
fn write_build_state(path: &Path, head: String, status_hash: String) {
    let state = BuildState {
        head,
        status_hash,
        built_at: now_ms(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&state) {
        let _ = std::fs::write(path, json);
    }
}

/// Get the current HEAD sha.
fn git_head(root: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["-C", &root.to_string_lossy(), "rev-parse", "HEAD"])
        .output()
        .context("git rev-parse HEAD")?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// SHA-256 of `git status --porcelain` output (content-independent of locale/timing).
fn git_status_hash(root: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["-C", &root.to_string_lossy(), "status", "--porcelain"])
        .output()
        .context("git status --porcelain")?;
    Ok(format!("{:x}", Sha256::digest(&output.stdout)))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Seed `graphify-out/` from a sibling worktree of the same repository.
///
/// On macOS, uses `cp -c -R` for copy-on-write cloning; elsewhere falls back to `cp -R`.
fn seed_from_sibling(root: &Path) -> Result<()> {
    // Find the common git dir (shared by all worktrees of the same repo).
    let common_dir_output = Command::new("git")
        .args(["-C", &root.to_string_lossy(), "rev-parse", "--git-common-dir"])
        .output()
        .context("git rev-parse --git-common-dir")?;
    if !common_dir_output.status.success() {
        anyhow::bail!("not a git repository");
    }
    let common_dir = root.join(String::from_utf8_lossy(&common_dir_output.stdout).trim());

    // List all worktrees.
    let wt_output = Command::new("git")
        .args([
            "-C",
            &root.to_string_lossy(),
            "worktree",
            "list",
            "--porcelain",
        ])
        .output()
        .context("git worktree list --porcelain")?;

    let wt_text = String::from_utf8_lossy(&wt_output.stdout);
    let mut worktree_paths: Vec<PathBuf> = Vec::new();
    for line in wt_text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            let p = PathBuf::from(path);
            if p != root {
                worktree_paths.push(p);
            }
        }
    }

    // Also check the main checkout (the parent of the common git dir, if it's a .git/ dir).
    let main_checkout = common_dir.parent();
    if let Some(main) = main_checkout.filter(|m| *m != root && !worktree_paths.contains(&m.to_path_buf())) {
        worktree_paths.push(main.to_path_buf());
    }

    // Find one with graphify-out/.
    let source = worktree_paths
        .iter()
        .find(|p| p.join("graphify-out").is_dir());
    let Some(source) = source else {
        return Ok(()); // No sibling has one; that's fine.
    };

    let source_out = source.join("graphify-out");
    let dest_out = root.join("graphify-out");

    let copy = |flags: &[&str]| {
        Command::new("cp")
            .args(flags)
            .arg(&source_out)
            .arg(&dest_out)
            .status()
            .is_ok_and(|s| s.success())
    };
    // Copy-on-write on macOS, else (or when that fails) a normal copy.
    if !((cfg!(target_os = "macos") && copy(&["-c", "-R"])) || copy(&["-R"])) {
        anyhow::bail!("cp of graphify-out failed");
    }

    // Rewrite .graphify_root to point to the new root.
    let root_file = dest_out.join(".graphify_root");
    if root_file.is_file() {
        let _ = std::fs::write(&root_file, root.to_string_lossy().as_bytes());
    }

    Ok(())
}

/// Keep `graphify-out/` out of `git status` via `.git/info/exclude`.
fn exclude_from_git(root: &Path) -> Result<()> {
    // Check if already ignored.
    let check = Command::new("git")
        .args([
            "-C",
            &root.to_string_lossy(),
            "check-ignore",
            "-q",
            "graphify-out/graph.json",
        ])
        .status();
    if check.map(|s| s.success()).unwrap_or(false) {
        return Ok(());
    }

    // Get the exclude file path.
    let output = Command::new("git")
        .args([
            "-C",
            &root.to_string_lossy(),
            "rev-parse",
            "--git-path",
            "info/exclude",
        ])
        .output()
        .context("git rev-parse --git-path info/exclude")?;
    if !output.status.success() {
        return Ok(());
    }
    let rel = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if rel.is_empty() {
        return Ok(());
    }
    let file = root.join(&rel);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let current = std::fs::read_to_string(&file).unwrap_or_default();
    let prefix = if !current.is_empty() && !current.ends_with('\n') {
        "\n"
    } else {
        ""
    };
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)?;
    write!(
        f,
        "{prefix}# Graphify's code graph, built by Co-Review\ngraphify-out/\n"
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_state_round_trip() {
        let state = BuildState {
            head: "abc123".to_string(),
            status_hash: "deadbeef".to_string(),
            built_at: 1234567890,
        };
        let json = serde_json::to_string(&state).expect("serialize");
        let back: BuildState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.head, state.head);
        assert_eq!(back.status_hash, state.status_hash);
        assert_eq!(back.built_at, state.built_at);
    }

    #[test]
    fn now_ms_is_reasonable() {
        let ms = now_ms();
        // Should be after 2020-01-01 in ms.
        assert!(ms > 1_577_836_800_000);
    }
}
