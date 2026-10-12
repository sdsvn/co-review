//! Post a Co-Review review to a GitHub pull request as one review through `gh api`.
//!
//! The payload building (`compose`, `preview`, `preview_markdown`) is pure: no I/O, no `gh`.
//! The `gh` interaction (`publish`) goes through a `GhRunner` trait so tests use a fake.

use std::collections::HashMap;
use std::io::Write as _;
use std::process::{Command, Stdio};

use anyhow::{bail, Context as _};
use review_model::{
    LocationKind, PatchAnchorType, PatchSide, ParticipantKind, ProposalStatus, Review,
    ReviewDecision, ReviewThread, ThreadIntent, ThreadStatus,
};
use serde::{Deserialize, Serialize};

/// The GitHub review event string corresponding to a Co-Review decision.
fn review_event(decision: ReviewDecision) -> &'static str {
    match decision {
        ReviewDecision::Approve => "APPROVE",
        ReviewDecision::RequestChanges => "REQUEST_CHANGES",
        ReviewDecision::Comment => "COMMENT",
    }
}

/// Human-readable label for a decision.
fn decision_label(decision: ReviewDecision) -> &'static str {
    match decision {
        ReviewDecision::Approve => "Approve",
        ReviewDecision::RequestChanges => "Request changes",
        ReviewDecision::Comment => "Comment",
    }
}

/// Matches `owner/repo` where each part is `[\w.-]+` (JavaScript's ASCII `\w`).
fn is_owner_repo(s: &str) -> bool {
    let valid = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    };
    s.split_once('/')
        .is_some_and(|(owner, name)| valid(owner) && valid(name))
}

#[derive(Debug, Clone, Serialize)]
struct LineComment {
    #[serde(skip_serializing)]
    thread_id: String,
    path: String,
    line: u32,
    side: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_side: Option<String>,
    body: String,
}

/// The pull request a review targets (from its review directory's `PR.md`).
#[derive(Debug, Clone)]
pub struct GitHubTarget {
    pub repo: String,
    pub number: u32,
    pub url: String,
    pub posted_round: Option<u32>,
    pub posted_url: Option<String>,
}

/// A line comment in the preview (exposed to callers).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewComment {
    pub thread_id: String,
    pub path: String,
    pub line: u32,
    pub start_line: Option<u32>,
    pub side: String,
    pub body: String,
}

/// Exactly what posting a review to GitHub would send.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GitHubPreview {
    pub event: String,
    pub body: String,
    pub comments: Vec<PreviewComment>,
}

/// The reviewer's edits to a preview.
#[derive(Debug, Clone, Deserialize)]
pub struct GitHubEdits {
    pub body: String,
    pub comments: HashMap<String, String>,
}

/// What posting a review to GitHub did.
#[derive(Debug, Clone)]
pub struct GitHubPost {
    pub url: String,
    pub comments: usize,
    pub in_body: usize,
    pub event: String,
    pub notes: Vec<String>,
}

/// Result of running the `gh` CLI.
#[derive(Debug, Clone)]
pub struct GhOutput {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Abstraction over the `gh` CLI so tests can inject a fake.
pub trait GhRunner {
    fn run(&self, args: &[&str], input: &str) -> GhOutput;
}

/// Default implementation that spawns `gh` as a child process.
pub struct DefaultGhRunner;

impl GhRunner for DefaultGhRunner {
    fn run(&self, args: &[&str], input: &str) -> GhOutput {
        let failed = |stderr: String| GhOutput {
            ok: false,
            stdout: String::new(),
            stderr,
        };
        let spawned = Command::new("gh")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => return failed(format!("ENOENT: {error}")),
        };
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input.as_bytes()).ok();
        }
        match child.wait_with_output() {
            Ok(output) => GhOutput {
                ok: output.status.success(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            },
            Err(error) => failed(error.to_string()),
        }
    }
}

/// Build the body of a thread's GitHub comment: the reviewer's point only, not the conversation.
/// Uses the rewording override when one was given, else the reviewer's own human messages;
/// a finding they accepted without writing anything uses the finding's text. Adds a GitHub
/// suggestion block for accepted (or pending) proposals on the new side.
fn thread_body(thread: &ReviewThread, reworded: Option<&str>) -> String {
    let text = if let Some(reworded) = reworded {
        Some(reworded.trim())
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    } else {
        let humans: Vec<String> = thread
            .messages
            .iter()
            .filter(|m| m.author.kind == ParticipantKind::Human && !m.body.trim().is_empty())
            .map(|m| m.body.trim().to_string())
            .collect();
        if !humans.is_empty() {
            Some(humans.join("\n\n"))
        } else {
            thread
                .messages
                .iter()
                .find(|m| m.author.kind != ParticipantKind::System && !m.body.trim().is_empty())
                .map(|m| m.body.trim().to_string())
        }
    };

    let text = text.unwrap_or_else(|| "(no message)".to_string());

    let suggestion = thread
        .proposal
        .as_ref()
        .filter(|p| p.status != ProposalStatus::Rejected)
        .filter(|_| {
            !thread
                .location
                .patch_anchor
                .as_ref()
                .is_some_and(|a| a.side == Some(PatchSide::Old))
        })
        .map(|p| {
            let after = p.after.strip_suffix('\n').unwrap_or(&p.after);
            format!("\n\n```suggestion\n{after}\n```")
        })
        .unwrap_or_default();

    format!("{text}{suggestion}")
}

/// Whether a thread belongs to the reviewer (not an agent's own thread).
fn is_reviewers(thread: &ReviewThread) -> bool {
    thread.intent != ThreadIntent::Question
        && (thread.origin.as_deref() == Some("finding")
            || thread.messages.iter().any(|m| m.author.kind == ParticipantKind::Human))
}

/// Whether a thread should be posted.
fn is_posted(thread: &ReviewThread) -> bool {
    (thread.status == ThreadStatus::Open && is_reviewers(thread))
        || thread
            .proposal
            .as_ref()
            .is_some_and(|p| p.status == ProposalStatus::Accepted)
}

/// The result of composing a review into GitHub's shape.
#[derive(Debug, Clone)]
pub struct ComposeResult {
    comments: Vec<LineComment>,
    /// The review body including comments that aren't on a diff line.
    pub body: String,
    /// How many comments ended up in the body instead of as line comments.
    pub elsewhere: usize,
}

/// Compose a review into line comments and a body. Pure function.
pub fn compose(
    review: &Review,
    decision: ReviewDecision,
    summary: &str,
    reworded: &HashMap<String, String>,
) -> ComposeResult {
    let mut comments: Vec<LineComment> = Vec::new();
    let mut elsewhere: Vec<String> = Vec::new();

    let mut threads: Vec<&ReviewThread> = review.threads.iter().filter(|t| is_posted(t)).collect();
    threads.sort_by_key(|t| t.number);

    for thread in threads {
        let anchor = thread.location.patch_anchor.as_ref();
        let side = anchor
            .and_then(|a| a.side.as_ref())
            .map_or("RIGHT", |s| match s {
                PatchSide::Old => "LEFT",
                PatchSide::New => "RIGHT",
            })
            .to_string();
        let body = thread_body(thread, reworded.get(&thread.id).map(|s| s.as_str()));

        // A diff-line comment: `(line, start_line)` of a line or a range of lines.
        let placed = anchor
            .filter(|_| thread.location.kind == LocationKind::Patch)
            .and_then(|a| {
                let lines = match a.anchor_type {
                    PatchAnchorType::CodeLine => a.line.map(|line| (line, None)),
                    PatchAnchorType::CodeRange => a
                        .start_line
                        .zip(a.end_line)
                        .map(|(start, end)| (end, (start != end).then_some(start))),
                    _ => None,
                };
                Some((a.path.clone()?, lines?))
            });
        if let Some((path, (line, start_line))) = placed {
            comments.push(LineComment {
                thread_id: thread.id.clone(),
                path,
                line,
                start_side: start_line.map(|_| side.clone()),
                side,
                start_line,
                body,
            });
            continue;
        }

        // Not a diff-line comment: goes into the body.
        let location_label = anchor
            .and_then(|a| a.path.as_deref())
            .unwrap_or_else(|| {
                if thread.location.doc_anchor.is_some() {
                    "the description"
                } else {
                    "the pull request"
                }
            });
        let indented_body = body.replace('\n', "\n  ");
        elsewhere.push(format!("- **{location_label}:** {indented_body}"));
    }

    let listed = elsewhere.join("\n");
    let mut body = [summary.trim(), listed.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if body.is_empty() && decision == ReviewDecision::Approve {
        body = "Looks good.".to_string();
    }

    ComposeResult {
        comments,
        body,
        elsewhere: elsewhere.len(),
    }
}

/// Build a preview of what would be posted. Pure function.
pub fn preview(
    review: &Review,
    decision: ReviewDecision,
    summary: &str,
    reworded: &HashMap<String, String>,
) -> GitHubPreview {
    let composed = compose(review, decision, summary, reworded);
    GitHubPreview {
        event: review_event(decision).to_string(),
        body: composed.body,
        comments: composed
            .comments
            .into_iter()
            .map(|c| PreviewComment {
                thread_id: c.thread_id,
                path: c.path,
                line: c.line,
                start_line: c.start_line,
                side: c.side,
                body: c.body,
            })
            .collect(),
    }
}

/// A preview as Markdown, for a thread that asks the reviewer to confirm a post.
pub fn preview_markdown(preview: &GitHubPreview) -> String {
    let quote = |text: &str| -> String {
        text.split('\n').map(|l| format!("> {l}")).collect::<Vec<_>>().join("\n")
    };

    let event_label = preview
        .event
        .to_lowercase()
        .replace('_', " ");
    let mut parts = Vec::new();

    if preview.body.trim().is_empty() {
        parts.push(format!("**Review ({event_label}):** no message"));
    } else {
        parts.push(format!(
            "**Review ({event_label}):**\n\n{}",
            quote(&preview.body)
        ));
    }

    for c in &preview.comments {
        let line_label = if let Some(start) = c.start_line {
            format!("{start}-")
        } else {
            String::new()
        };
        let side_label = if c.side == "LEFT" { " (old side)" } else { "" };
        parts.push(format!(
            "**{}:{line_label}{}{side_label}:**\n\n{}",
            c.path,
            c.line,
            quote(&c.body)
        ));
    }

    parts.join("\n\n")
}

/// Post the review to GitHub. Handles retries for own-PR and diff-line errors.
pub fn publish(
    runner: &dyn GhRunner,
    review: &Review,
    target: &GitHubTarget,
    decision: ReviewDecision,
    summary: &str,
    reworded: &HashMap<String, String>,
    edits: Option<&GitHubEdits>,
) -> anyhow::Result<GitHubPost> {
    let composed = compose(review, decision, summary, reworded);
    let endpoint = format!("repos/{}/pulls/{}/reviews", target.repo, target.number);
    let mut event = review_event(decision).to_string();
    let mut body = edits.map_or_else(|| composed.body.clone(), |e| e.body.trim().to_string());
    let mut comments: Vec<LineComment> = composed
        .comments
        .into_iter()
        .map(|c| match edits.and_then(|e| e.comments.get(&c.thread_id)) {
            Some(edited) => LineComment {
                body: edited.trim().to_string(),
                ..c
            },
            None => c,
        })
        // The reviewer emptied it in the preview.
        .filter(|c| !c.body.is_empty())
        .collect();

    let posted = comments.len();
    let mut notes: Vec<String> = Vec::new();

    for _attempt in 0..3 {
        let payload = serde_json::json!({ "event": event, "body": body, "comments": comments });
        let input = serde_json::to_string(&payload).context("serializing review payload")?;
        let result = runner.run(
            &["api", "--method", "POST", &endpoint, "--input", "-"],
            &input,
        );

        if result.ok {
            let url = serde_json::from_str::<serde_json::Value>(&result.stdout)
                .ok()
                .and_then(|v| v.get("html_url")?.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| target.url.clone());

            return Ok(GitHubPost {
                url,
                comments: comments.len(),
                in_body: composed.elsewhere + if comments.is_empty() { posted } else { 0 },
                event,
                notes,
            });
        }

        let message = if result.stderr.is_empty() {
            &result.stdout
        } else {
            &result.stderr
        };
        let message_lower = message.to_lowercase();

        if message_lower.contains("enoent")
            || message_lower.contains("not found: gh")
            || message_lower.contains("command not found")
        {
            bail!("The GitHub CLI (`gh`) is not installed. Install it (https://cli.github.com) and run `gh auth login`.");
        }

        if message_lower.contains("gh auth login")
            || message_lower.contains("authentication")
            || message_lower.contains("http 401")
        {
            bail!(
                "The GitHub CLI is not signed in: run `gh auth login`, then post again."
            );
        }

        if event != "COMMENT" && message_lower.contains("own pull request") {
            let label = decision_label(decision);
            notes.push(format!(
                "GitHub doesn't allow \"{label}\" on your own pull request, so it was posted as a comment."
            ));
            body = format!("**{label}.** {body}").trim_end().to_string();
            event = "COMMENT".to_string();
            continue;
        }

        if !comments.is_empty()
            && (message_lower.contains("http 422")
                || message_lower.contains("unprocessable")
                || message_lower.contains("line")
                || message_lower.contains("position")
                || message_lower.contains("diff"))
        {
            notes.push(
                "Some lines are no longer in the pull request's diff, so the line comments were added to the review's body.".to_string(),
            );
            let comment_lines: Vec<String> = comments
                .iter()
                .map(|c| {
                    let line_prefix = c
                        .start_line
                        .map(|sl| format!("{sl}-"))
                        .unwrap_or_default();
                    let indented = c.body.replace('\n', "\n  ");
                    format!("- **{}:{line_prefix}{}:** {indented}", c.path, c.line)
                })
                .collect();
            let parts: Vec<&str> = std::iter::once(body.as_str())
                .chain(comment_lines.iter().map(|s| s.as_str()))
                .filter(|s| !s.is_empty())
                .collect();
            body = parts.join("\n");
            comments.clear();
            continue;
        }

        // The first 400 UTF-16 code units, as JavaScript's `.slice(0, 400)`.
        let mut units = 0;
        let truncated: String = message
            .trim()
            .chars()
            .take_while(|c| {
                units += c.len_utf16();
                units <= 400
            })
            .collect();
        bail!("GitHub refused the review: {truncated}");
    }

    bail!("GitHub refused the review.")
}

/// Read the PR metadata from a review directory's `PR.md` or `<patch>.md`.
/// Returns `None` if the directory does not contain a valid PR reference.
pub fn resolve_target(
    dir: &std::path::Path,
    review: &Review,
) -> Option<GitHubTarget> {
    let patches = review_bundle::service::patches_of(dir);
    let candidates = std::iter::once(dir.join("PR.md")).chain(patches.iter().map(|patch| patch.with_extension("md")));
    for path in candidates {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let meta = review_bundle::patch::parse_patch_meta(&text);
        if let (Some(number), Some(repo)) = (meta.number, meta.repo.as_deref())
            && is_owner_repo(repo)
        {
            let posted = review.bundle.as_ref().and_then(|b| b.github.as_ref());
            return Some(GitHubTarget {
                url: meta
                    .url
                    .unwrap_or_else(|| format!("https://github.com/{repo}/pull/{number}")),
                repo: repo.to_string(),
                number,
                posted_round: posted.map(|g| g.round),
                posted_url: posted.map(|g| g.url.clone()),
            });
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    /// Load a fixture review JSON.
    fn load_review(name: &str) -> Review {
        let path = fixtures_dir().join(format!("{name}.review.json"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()));
        serde_json::from_str(&text).unwrap()
    }

    /// Load the expected preview JSON the TypeScript produced.
    fn load_expected_preview(name: &str) -> GitHubPreview {
        let path = fixtures_dir().join(format!("{name}.preview.json"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()));
        serde_json::from_str(&text).unwrap()
    }

    /// Load the expected preview markdown.
    fn load_expected_markdown(name: &str) -> String {
        let path = fixtures_dir().join(format!("{name}.preview.md"));
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()))
    }

    /// Run a compose/preview test against a fixture: same review in, same preview out.
    fn assert_preview_matches(name: &str) {
        let review = load_review(name);
        let decision = review.verdict.as_ref().map(|v| v.decision).unwrap_or(ReviewDecision::Comment);
        let summary = review.verdict.as_ref().map(|v| v.summary.as_str()).unwrap_or("");
        let reworded = HashMap::new();
        let actual = preview(&review, decision, summary, &reworded);
        let expected = load_expected_preview(name);
        assert_eq!(actual, expected, "preview mismatch for fixture {name}");
    }

    fn assert_markdown_matches(name: &str) {
        let expected_preview = load_expected_preview(name);
        let actual = preview_markdown(&expected_preview);
        let expected = load_expected_markdown(name);
        assert_eq!(actual.trim(), expected.trim(), "markdown mismatch for fixture {name}");
    }

    // --- Fixture-driven tests ---

    #[test]
    fn test_basic_line_comment() {
        assert_preview_matches("basic_line");
    }

    #[test]
    fn test_basic_line_markdown() {
        assert_markdown_matches("basic_line");
    }

    #[test]
    fn test_code_range() {
        assert_preview_matches("code_range");
    }

    #[test]
    fn test_reworded() {
        // Reworded test uses a custom reworded map.
        let review = load_review("reworded");
        let decision = review.verdict.as_ref().map(|v| v.decision).unwrap_or(ReviewDecision::Comment);
        let summary = review.verdict.as_ref().map(|v| v.summary.as_str()).unwrap_or("");
        let reworded_path = fixtures_dir().join("reworded.reworded.json");
        let reworded: HashMap<String, String> =
            serde_json::from_str(&std::fs::read_to_string(reworded_path).unwrap()).unwrap();
        let actual = preview(&review, decision, summary, &reworded);
        let expected = load_expected_preview("reworded");
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_suggestion() {
        assert_preview_matches("suggestion");
    }

    #[test]
    fn test_elsewhere() {
        assert_preview_matches("elsewhere");
    }

    #[test]
    fn test_filtered_out() {
        assert_preview_matches("filtered_out");
    }

    #[test]
    fn test_approve_default_body() {
        assert_preview_matches("approve_default");
    }

    /// A `gh` that fails the first post with `error` (when given) and accepts the next one.
    struct FakeGh {
        error: Option<&'static str>,
        payloads: std::sync::Mutex<Vec<serde_json::Value>>,
    }

    impl GhRunner for FakeGh {
        fn run(&self, _args: &[&str], input: &str) -> GhOutput {
            let mut payloads = self.payloads.lock().unwrap();
            payloads.push(serde_json::from_str(input).unwrap());
            match self.error.filter(|_| payloads.len() == 1) {
                Some(error) => GhOutput {
                    ok: false,
                    stdout: String::new(),
                    stderr: error.to_string(),
                },
                None => GhOutput {
                    ok: true,
                    stdout: r#"{"html_url":"https://github.com/owner/repo/pull/42#pullrequestreview-1"}"#.to_string(),
                    stderr: String::new(),
                },
            }
        }
    }

    /// Publishes a fixture's review through a fake `gh`; the payloads sent must be the ones the TypeScript sent.
    fn publish_fixture(name: &str, error: Option<&'static str>, edits: Option<&GitHubEdits>) -> GitHubPost {
        let review = load_review(name);
        let target = GitHubTarget {
            repo: "owner/repo".to_string(),
            number: 42,
            url: "https://github.com/owner/repo/pull/42".to_string(),
            posted_round: None,
            posted_url: None,
        };
        let verdict = review.verdict.as_ref().unwrap();
        let runner = FakeGh {
            error,
            payloads: std::sync::Mutex::new(Vec::new()),
        };
        let result = publish(&runner, &review, &target, verdict.decision, &verdict.summary, &HashMap::new(), edits).unwrap();
        let path = fixtures_dir().join(format!("{name}.payloads.json"));
        let expected: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(runner.payloads.into_inner().unwrap(), expected, "payloads mismatch for fixture {name}");
        result
    }

    #[test]
    fn test_own_pr_fallback() {
        let result = publish_fixture("own_pr", Some("You can't approve your own pull request"), None);
        assert_eq!(result.event, "COMMENT");
        assert!(result.notes[0].contains("your own pull request"));
    }

    #[test]
    fn test_diff_line_fallback() {
        let error = "HTTP 422: Unprocessable Entity - line must be part of the diff";
        let result = publish_fixture("diff_fallback", Some(error), None);
        assert_eq!(result.comments, 0);
        assert!(result.in_body > 0);
        assert!(result.notes[0].contains("no longer in the pull request's diff"));
    }

    #[test]
    fn test_edits() {
        // The reviewer changes the summary, rewrites one comment and empties another.
        let path = fixtures_dir().join("edits.edits.json");
        let edits: GitHubEdits = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let result = publish_fixture("edits", None, Some(&edits));
        // The emptied comment is left out: 3 threads composed, 2 posted.
        assert_eq!(result.comments, 2);
        assert_eq!(result.event, "REQUEST_CHANGES");
    }
}
