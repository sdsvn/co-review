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
    /// Why the event isn't the reviewer's decision (on your own pull request GitHub only takes a comment).
    #[serde(default)]
    pub note: Option<String>,
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

/// GitHub takes only a comment on your own pull request: the review's body says the decision instead.
fn as_comment(decision: ReviewDecision, body: &str) -> String {
    format!("**{}.** {body}", decision_label(decision)).trim().to_string()
}

fn own_note(decision: ReviewDecision) -> String {
    format!(
        "GitHub doesn't allow \"{}\" on your own pull request, so it is posted as a comment that says so.",
        decision_label(decision)
    )
}

/// Whether the pull request is the signed-in GitHub user's own: GitHub refuses "Approve" and "Request changes" on it.
/// `None` when `gh` can't tell (the post then finds out from GitHub's refusal). Runs `gh`, so call it off the UI thread.
pub fn own_pull_request(runner: &dyn GhRunner, target: &GitHubTarget) -> Option<bool> {
    let pull = format!("repos/{}/pulls/{}", target.repo, target.number);
    let author = runner.run(&["api", &pull, "--jq", ".user.login"], "");
    let me = runner.run(&["api", "user", "--jq", ".login"], "");
    let (author_login, my_login) = (author.stdout.trim(), me.stdout.trim());
    (author.ok && me.ok && !author_login.is_empty()).then(|| author_login == my_login)
}

/// Build a preview of what would be posted: exactly what `publish` sends first. Pure function; `own` is whether the
/// pull request is the reviewer's own (`own_pull_request`), where an Approve or Request changes goes as a comment that
/// says so.
pub fn preview(
    review: &Review,
    decision: ReviewDecision,
    summary: &str,
    reworded: &HashMap<String, String>,
    own: bool,
) -> GitHubPreview {
    let composed = compose(review, decision, summary, reworded);
    let own = own && decision != ReviewDecision::Comment;
    GitHubPreview {
        event: if own { "COMMENT" } else { review_event(decision) }.to_string(),
        body: if own { as_comment(decision, &composed.body) } else { composed.body },
        note: own.then(|| own_note(decision)),
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

    if let Some(note) = preview.note.as_deref() {
        parts.push(format!("_{note}_"));
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

/// Post the review to GitHub. Checks first whether the pull request is the reviewer's own (then an Approve or Request
/// changes goes as a comment that says so); GitHub's refusals that have a fallback (your own pull request, a line
/// outside the diff) are retried, each once. `edits` are the reviewer's changes to its preview, posted as they wrote them.
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
    // The reviewer's edits are of the preview, which already reads as a comment on their own pull request.
    let as_own = |notes: &mut Vec<String>, body: &mut String, event: &mut String| {
        notes.push(own_note(decision));
        if edits.is_none() {
            *body = as_comment(decision, body);
        }
        *event = "COMMENT".to_string();
    };
    if event != "COMMENT" && own_pull_request(runner, target) == Some(true) {
        as_own(&mut notes, &mut body, &mut event);
    }

    // Each fallback once: your own pull request, then lines no longer in the diff.
    let mut last = String::new();
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

        // `gh api` puts GitHub's reason on stderr (`gh: <reason>`) and the response itself on stdout.
        let (stderr, stdout) = (result.stderr.trim(), result.stdout.trim());
        let message = [stderr, stdout]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        last = if stderr.is_empty() { stdout } else { stderr }.to_string();
        log::error!(
            "GitHub refused the review of {}#{} ({event}, {} line comments): {message}",
            target.repo,
            target.number,
            comments.len()
        );
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
            // GitHub doesn't let you approve (or request changes on) your own pull request.
            as_own(&mut notes, &mut body, &mut event);
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

        bail!("GitHub refused the review: {}", refusal(&last));
    }

    bail!("GitHub refused the review: {}", refusal(&last))
}

/// GitHub's reason for a refusal, as the error names it: without `gh`'s prefix, the first 400 UTF-16 code units (as
/// JavaScript's `.slice(0, 400)`).
fn refusal(last: &str) -> String {
    let reason = last.strip_prefix("gh: ").unwrap_or(last);
    let mut units = 0;
    reason
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= 400
        })
        .collect()
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
        let actual = preview(&review, decision, summary, &reworded, false);
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
        let actual = preview(&review, decision, summary, &reworded, false);
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

    /// A `gh` whose `n`th post fails with `errors[n - 1]` (when there is one) and is accepted otherwise. Asked who
    /// wrote the pull request and who is signed in, it answers `logins` (author, user), or fails when there are none.
    struct FakeGh {
        logins: Option<(&'static str, &'static str)>,
        errors: Vec<&'static str>,
        payloads: std::sync::Mutex<Vec<serde_json::Value>>,
    }

    impl FakeGh {
        fn new(logins: Option<(&'static str, &'static str)>, errors: Vec<&'static str>) -> Self {
            FakeGh {
                logins,
                errors,
                payloads: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl GhRunner for FakeGh {
        fn run(&self, args: &[&str], input: &str) -> GhOutput {
            let answer = |ok: bool, stdout: &str, stderr: &str| GhOutput {
                ok,
                stdout: stdout.to_string(),
                stderr: stderr.to_string(),
            };
            if args.get(1) != Some(&"--method") {
                let login = match (self.logins, args.get(1)) {
                    (Some((_, me)), Some(&"user")) => me,
                    (Some((author, _)), Some(&"repos/owner/repo/pulls/42")) => author,
                    _ => return answer(false, "", "gh: Not Found (HTTP 404)"),
                };
                return answer(true, &format!("{login}\n"), "");
            }
            let mut payloads = self.payloads.lock().unwrap();
            payloads.push(serde_json::from_str(input).unwrap());
            match self.errors.get(payloads.len() - 1) {
                Some(error) => answer(false, "", error),
                None => answer(true, r#"{"html_url":"https://github.com/owner/repo/pull/42#pullrequestreview-1"}"#, ""),
            }
        }
    }

    fn target() -> GitHubTarget {
        GitHubTarget {
            repo: "owner/repo".to_string(),
            number: 42,
            url: "https://github.com/owner/repo/pull/42".to_string(),
            posted_round: None,
            posted_url: None,
        }
    }

    /// Publishes a fixture's review through a fake `gh`.
    fn publish_with(name: &str, runner: &FakeGh, edits: Option<&GitHubEdits>) -> anyhow::Result<GitHubPost> {
        let review = load_review(name);
        let verdict = review.verdict.as_ref().unwrap();
        publish(runner, &review, &target(), verdict.decision, &verdict.summary, &HashMap::new(), edits)
    }

    /// Publishes a fixture's review through a fake `gh`; the payloads sent must be the ones the TypeScript sent.
    fn publish_fixture(name: &str, runner: FakeGh, edits: Option<&GitHubEdits>) -> GitHubPost {
        let result = publish_with(name, &runner, edits).unwrap();
        let path = fixtures_dir().join(format!("{name}.payloads.json"));
        let expected: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(runner.payloads.into_inner().unwrap(), expected, "payloads mismatch for fixture {name}");
        result
    }

    const OWN_NOTE: &str = "GitHub doesn't allow \"Approve\" on your own pull request, so it is posted as a comment that says so.";

    #[test]
    fn test_own_pr_fallback() {
        // `gh` can't tell whose pull request it is: GitHub's refusal is what turns it into a comment.
        let runner = FakeGh::new(None, vec!["gh: Can not approve your own pull request (HTTP 422)"]);
        let result = publish_fixture("own_pr", runner, None);
        assert_eq!(result.event, "COMMENT");
        assert_eq!(result.notes, vec![OWN_NOTE.to_string()]);
    }

    #[test]
    fn test_own_pr_checked_first() {
        // The reviewer's own pull request: posted as a comment that says so, without being refused first.
        let result = publish_fixture("own_pr_checked", FakeGh::new(Some(("me", "me")), vec![]), None);
        assert_eq!(result.event, "COMMENT");
        assert_eq!(result.notes, vec![OWN_NOTE.to_string()]);
    }

    #[test]
    fn test_someone_elses_pr_keeps_decision() {
        let runner = FakeGh::new(Some(("someone", "me")), vec![]);
        let result = publish_with("own_pr", &runner, None).unwrap();
        assert_eq!(result.event, "APPROVE");
        assert!(result.notes.is_empty());
        assert_eq!(runner.payloads.into_inner().unwrap().len(), 1);
    }

    #[test]
    fn test_own_pr_preview() {
        let review = load_review("own_pr_checked");
        let verdict = review.verdict.as_ref().unwrap();
        let actual = preview(&review, verdict.decision, &verdict.summary, &HashMap::new(), true);
        assert_eq!(actual, load_expected_preview("own_pr_checked"));
        assert_markdown_matches("own_pr_checked");
        // A comment is a comment on any pull request.
        let comment = preview(&review, ReviewDecision::Comment, &verdict.summary, &HashMap::new(), true);
        assert_eq!((comment.event.as_str(), comment.note), ("COMMENT", None));
    }

    #[test]
    fn test_own_pr_edits_posted_as_written() {
        // The edited preview already reads as a comment: the edits go out as the reviewer wrote them.
        let path = fixtures_dir().join("edits.edits.json");
        let edits: GitHubEdits = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let runner = FakeGh::new(Some(("me", "me")), vec![]);
        let result = publish_with("edits", &runner, Some(&edits)).unwrap();
        assert_eq!(result.event, "COMMENT");
        let payloads = runner.payloads.into_inner().unwrap();
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0]["event"], "COMMENT");
        assert_eq!(payloads[0]["body"], "Edited summary by reviewer.");
    }

    #[test]
    fn test_each_fallback_once_and_github_reason() {
        // Own pull request, then a line outside the diff, then a refusal without a fallback: the error is GitHub's reason.
        let runner = FakeGh::new(
            None,
            vec![
                "gh: Can not approve your own pull request (HTTP 422)",
                "gh: Pull request review thread line must be part of the diff (HTTP 422)",
                "gh: Validation Failed (HTTP 422)",
            ],
        );
        let error = publish_with("own_pr", &runner, None).unwrap_err().to_string();
        assert_eq!(error, "GitHub refused the review: Validation Failed (HTTP 422)");
        let payloads = runner.payloads.into_inner().unwrap();
        assert_eq!(payloads.len(), 3);
        assert_eq!(payloads[1]["event"], "COMMENT");
        assert_eq!(payloads[2]["comments"], serde_json::json!([]));
    }

    #[test]
    fn test_own_pr_refused_again_is_not_retried() {
        // Already a comment: a second "own pull request" refusal has no fallback left.
        let runner = FakeGh::new(None, vec!["gh: Can not approve your own pull request", "gh: Can not approve your own pull request"]);
        let error = publish_with("approve_default", &runner, None).unwrap_err().to_string();
        assert_eq!(error, "GitHub refused the review: Can not approve your own pull request");
        assert_eq!(runner.payloads.into_inner().unwrap().len(), 2);
    }

    #[test]
    fn test_diff_line_fallback() {
        let error = "HTTP 422: Unprocessable Entity - line must be part of the diff";
        let result = publish_fixture("diff_fallback", FakeGh::new(None, vec![error]), None);
        assert_eq!(result.comments, 0);
        assert!(result.in_body > 0);
        assert!(result.notes[0].contains("no longer in the pull request's diff"));
    }

    #[test]
    fn test_edits() {
        // The reviewer changes the summary, rewrites one comment and empties another.
        let path = fixtures_dir().join("edits.edits.json");
        let edits: GitHubEdits = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let result = publish_fixture("edits", FakeGh::new(Some(("someone", "me")), vec![]), Some(&edits));
        // The emptied comment is left out: 3 threads composed, 2 posted.
        assert_eq!(result.comments, 2);
        assert_eq!(result.event, "REQUEST_CHANGES");
    }
}
