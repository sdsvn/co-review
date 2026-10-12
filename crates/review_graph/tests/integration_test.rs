//! Integration test for build_graph: cold build, second worktree seeded from the first, restart.
//!
//! Ignored by default (requires `graphify` to be installed and takes seconds).
//! Run with: cargo test -p review_graph --test integration_test -- --ignored

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use review_graph::{build_graph, GraphState};

fn repo_root() -> PathBuf {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .expect("git rev-parse works");
    PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
}

fn add_worktree(base: &Path, name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("co-review-integ-{name}"));
    // Clean up any leftover from a previous run.
    let _ = Command::new("git")
        .args([
            "-C",
            &base.to_string_lossy(),
            "worktree",
            "remove",
            "--force",
            &path.to_string_lossy(),
        ])
        .output();
    let _ = std::fs::remove_dir_all(&path);

    let status = Command::new("git")
        .args([
            "-C",
            &base.to_string_lossy(),
            "worktree",
            "add",
            "--detach",
            &path.to_string_lossy(),
        ])
        .status()
        .expect("git worktree add works");
    assert!(status.success(), "git worktree add failed");
    path
}

fn remove_worktree(base: &Path, wt: &Path) {
    // Remove graphify-out first to avoid leaving large directories.
    let _ = std::fs::remove_dir_all(wt.join("graphify-out"));
    let _ = Command::new("git")
        .args([
            "-C",
            &base.to_string_lossy(),
            "worktree",
            "remove",
            "--force",
            &wt.to_string_lossy(),
        ])
        .status();
    let _ = std::fs::remove_dir_all(wt);
}

#[test]
#[ignore]
fn cold_build_seeded_build_and_restart() {
    let root = repo_root();

    // Create first worktree for cold build.
    let wt1 = add_worktree(&root, "wt1");
    eprintln!("worktree 1: {}", wt1.display());

    // Cold build.
    let start = Instant::now();
    let state = build_graph(&wt1);
    let cold_ms = start.elapsed().as_millis();
    eprintln!("cold build: {cold_ms} ms, state: {state:?}");
    assert_eq!(state, GraphState::Built, "cold build should succeed");
    assert!(
        wt1.join("graphify-out/graph.json").is_file(),
        "graph.json should exist after cold build"
    );

    // Create second worktree and seed from wt1.
    let wt2 = add_worktree(&root, "wt2");
    eprintln!("worktree 2: {}", wt2.display());

    let start = Instant::now();
    let state = build_graph(&wt2);
    let seeded_ms = start.elapsed().as_millis();
    eprintln!("seeded build: {seeded_ms} ms, state: {state:?}");
    assert_eq!(state, GraphState::Built, "seeded build should succeed");
    assert!(
        wt2.join("graphify-out/graph.json").is_file(),
        "graph.json should exist after seeded build"
    );

    // Restart (no changes) on wt1.
    let start = Instant::now();
    let state = build_graph(&wt1);
    let restart_ms = start.elapsed().as_millis();
    eprintln!("restart (no changes): {restart_ms} ms, state: {state:?}");
    assert_eq!(state, GraphState::Built, "restart should succeed");

    // Report.
    eprintln!("--- Timings ---");
    eprintln!("Cold build:      {cold_ms} ms");
    eprintln!("Seeded build:    {seeded_ms} ms");
    eprintln!("Restart (skip):  {restart_ms} ms");

    // The restart should be fast (no graphify run).
    assert!(
        restart_ms < 2000,
        "restart with no changes should be fast (was {restart_ms} ms)"
    );

    // Clean up.
    remove_worktree(&root, &wt1);
    remove_worktree(&root, &wt2);
}
