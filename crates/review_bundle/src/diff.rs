//! The diff `open_review({ diff })` reviews: Co-Review runs `git diff` itself (untracked files
//! included for `HEAD`) and writes the patch into the review directory.
//! Port of `gitDiff` and the `diff` branch of `open_review` in `co-reviewer-mcp.ts`.

use anyhow::{Context as _, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::payloads::resolve_path;

/// A revision range git can take without it being read as an option: `[\w./~^@{}-]+`.
fn is_valid_range(range: &str) -> bool {
    !range.is_empty()
        && !range.starts_with('-')
        && range.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | '.' | '/' | '~' | '^' | '@' | '{' | '}' | '-')
        })
}

/// What `open_review({ diff })` hands to `BundleService::open`: the diff written to
/// `<dir>/<patch_name or "change">.patch` when there is a directory, else as inline patch text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TakenDiff {
    Written(PathBuf),
    Inline(String),
}

/// Runs `git diff <range>` in `repo` for a review. The errors are the texts `open_review` returns.
pub fn take_diff(
    repo: &Path,
    range: &str,
    dir: Option<&Path>,
    patch_name: Option<&str>,
) -> Result<(String, TakenDiff)> {
    let Some(text) = git_diff(repo, range) else {
        bail!(
            "git diff {range} failed in {}: is it a git repository, and is \"{range}\" a valid revision range?",
            repo.display()
        );
    };
    if text.trim().is_empty() {
        bail!("git diff {range} is empty: there is nothing to review.");
    }
    let Some(dir) = dir else {
        return Ok((text.clone(), TakenDiff::Inline(text)));
    };
    let dir = resolve_path(dir);
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let name = patch_name
        .filter(|name| !name.is_empty())
        .unwrap_or("change");
    let file = dir.join(format!("{name}.patch"));
    fs::write(&file, &text).with_context(|| format!("writing {}", file.display()))?;
    Ok((text, TakenDiff::Written(file)))
}

/// `git diff <range>` in `repo` as plain text (no pager, colours or external diff tools), or
/// `None` when git fails. For `HEAD` (the uncommitted work) new, untracked files are included
/// too, without touching the index.
fn git_diff(repo: &Path, range: &str) -> Option<String> {
    if !is_valid_range(range) {
        return None;
    }
    let mut diff = run_git(
        repo,
        &["diff", "--no-color", "--no-ext-diff", range, "--"],
        &[0],
    )?;
    if range != "HEAD" {
        return Some(diff);
    }
    let untracked = run_git(
        repo,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        &[0],
    )
    .unwrap_or_default();
    for file in untracked.split('\0').filter(|file| !file.is_empty()) {
        // `diff --no-index` exits 1 when the files differ, which they always do here.
        let arguments = [
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-index",
            "--",
            "/dev/null",
            file,
        ];
        if let Some(added) = run_git(repo, &arguments, &[0, 1]) {
            diff.push_str(&added);
        }
    }
    Some(diff)
}

/// git's output, or `None` when git cannot run or exits with a code outside `ok_codes`.
fn run_git(repo: &Path, args: &[&str], ok_codes: &[i32]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("--no-pager")
        .args(args)
        .output()
        .ok()?;
    (output.status.success()
        || output
            .status
            .code()
            .is_some_and(|code| ok_codes.contains(&code)))
    .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(status.status.success(), "{:?}", status);
    }

    #[test]
    fn head_diff_includes_untracked_files_and_is_written_into_the_directory() {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-q"]);
        fs::write(repo.path().join("a.txt"), "one\n").unwrap();
        git(repo.path(), &["add", "a.txt"]);
        git(repo.path(), &["commit", "-q", "-m", "first"]);
        fs::write(repo.path().join("a.txt"), "two\n").unwrap();
        fs::write(repo.path().join("new.txt"), "fresh\n").unwrap();

        let out = tempfile::tempdir().unwrap();
        let dir = out.path().join("review");
        let (text, taken) = take_diff(repo.path(), "HEAD", Some(&dir), Some("work")).unwrap();
        assert!(text.contains("-one\n+two"));
        assert!(text.contains("+++ b/new.txt"));
        assert_eq!(taken, TakenDiff::Written(dir.join("work.patch")));
        assert_eq!(fs::read_to_string(dir.join("work.patch")).unwrap(), text);

        let (_, taken) = take_diff(repo.path(), "HEAD", None, None).unwrap();
        assert_eq!(taken, TakenDiff::Inline(text));

        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "second"]);
        let error = take_diff(repo.path(), "HEAD", None, None).unwrap_err();
        assert_eq!(
            error.to_string(),
            "git diff HEAD is empty: there is nothing to review."
        );
        let error = take_diff(repo.path(), "--stat", None, None).unwrap_err();
        assert!(error.to_string().starts_with("git diff --stat failed in "));
        let error = take_diff(repo.path(), "no-such-branch", None, None).unwrap_err();
        assert!(error.to_string().contains("valid revision range"));
    }

    #[test]
    fn test_valid_range() {
        assert!(is_valid_range("feature_x...HEAD"));
        assert!(is_valid_range("HEAD"));
        assert!(is_valid_range("main...HEAD"));
        assert!(is_valid_range("abc123~2"));
        assert!(is_valid_range("v1.0.0"));
        assert!(!is_valid_range("--stat"));
        assert!(!is_valid_range("-p"));
        assert!(!is_valid_range(""));
    }
}
