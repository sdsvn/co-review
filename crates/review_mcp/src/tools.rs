//! The tools as agents see them in `tools/list`: names, descriptions and input schemas, as the Theia app declares
//! them (its zod schemas, as JSON Schema).

use serde_json::{Value, json};

use crate::prompts::{ANSWER_STYLE_SHORT, DESIGN_PROMPT};

const ROOT: &str =
    "Absolute path of the repository (defaults to the one Co-Review was started for)";
const REVIEW: &str = "Review id (defaults to the review opened with open_review)";

/// Every tool: `(name, description, input schema)`.
pub fn definitions() -> Vec<(&'static str, String, Value)> {
    vec![
        ("open_review", open_review_description(), schema(open_review_properties(), &[])),
        (
            "await_reviewer",
            concat!(
                "Wait for the reviewer's next move; the one call to wait on, whatever you are waiting for. Returns ",
                "{status:\"comment\", threads[] (the ones to answer, with code and conversation), comments[] (unresolved threads), needsReply} ",
                "when they ask you something, reply in a thread you are part of (in a review directory: any comment, including on ",
                "proposed findings) or answer an ask_reviewer question; {status:\"submitted\", decision, summary, comments[], ",
                "acceptedSuggestions[]} when they click Submit; {status:\"closed\", hint} when they closed the review window or quit ",
                "Co-Review (stop waiting on it and tell the user); {status:\"pending\"} on timeout. Answer comments with reply, act on a ",
                "Submit, and call it again until the review is done."
            )
            .to_string(),
            schema(
                json!({
                    "reviewId": described(REVIEW),
                    "timeoutSec": { "type": "integer", "minimum": 1, "maximum": 3600 },
                }),
                &[],
            ),
        ),
        (
            "reply",
            format!("Answer in a review thread (markdown). {ANSWER_STYLE_SHORT} threadId: the thread id or its short id (\"t2\")."),
            schema(
                json!({
                    "threadId": { "type": "string" },
                    "body": { "type": "string" },
                    "resolve": { "type": "boolean" },
                    "reviewId": described(REVIEW),
                }),
                &["threadId", "body"],
            ),
        ),
        (
            "add_findings",
            concat!(
                "Add findings. Two shapes: {path, line?, endLine?, body, severity?, labels?, status?} creates threads on repository code ",
                "(without `line`, on the file or folder; `labels` group them in the panel, e.g. the area of the repository; ",
                "`status: \"proposed\"` lets the reviewer Accept or Dismiss each one, as for a first-pass audit); ",
                "anchored findings {id, target (\"doc\" | \"doc:<path>\" | \"patch:<slug>\"), anchor, severity, labels, body, verdict?} are PROPOSED ",
                "(the reviewer Accepts or Dismisses; only accepted ones come back in the submitted batch). A Markdown page takes ",
                "target \"doc:<path>\" and anchor {type: \"text\", exact} | {type: \"document\"}."
            )
            .to_string(),
            schema(
                json!({
                    "reviewId": described(REVIEW),
                    "findings": { "type": "array", "items": finding_schema() },
                }),
                &["findings"],
            ),
        ),
        (
            "ask_reviewer",
            concat!(
                "Ask the reviewer to decide between options (shown as buttons in the thread), then wait as await_reviewer does. ",
                "Their choice comes back as their message in that thread ({status:\"comment\"}, `question.choice` set); if they comment ",
                "elsewhere, reply in the thread or submit first, you get that instead, and the choice comes later from await_reviewer."
            )
            .to_string(),
            schema(
                json!({
                    "reviewId": described(REVIEW),
                    "question": { "type": "string" },
                    "options": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 6 },
                    "threadId": described("Ask inside this thread; otherwise a repository-level thread is created"),
                    "timeoutSec": { "type": "integer", "minimum": 1, "maximum": 3600 },
                }),
                &["question", "options"],
            ),
        ),
        (
            "post_review_to_github",
            concat!(
                "Post the review to its GitHub pull request (a review directory whose PR.md has `repo: <owner>/<name>` and ",
                "`pr: <number>`), as one GitHub review: open threads on diff lines become line comments, each the reviewer's point ",
                "only, not the discussion (suggested edits become GitHub suggestions), the rest goes into the review's body, and the reviewer's latest verdict is the review's decision. ",
                "Only when the reviewer asks for it: Co-Review first shows them what will be posted and posts only if they confirm."
            )
            .to_string(),
            schema(
                json!({
                    "reviewId": described(REVIEW),
                    "summary": described("The review's message on GitHub (default: the reviewer's summary from Submit review)"),
                    "comments": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": { "threadId": { "type": "string" }, "body": { "type": "string" } },
                            "required": ["threadId", "body"],
                            "additionalProperties": false,
                        },
                        "description": concat!(
                            "Each open comment as the ",
                            "reviewer's own point, in their voice (threadId: its id or short id). Only what they want changed, as one comment ",
                            "of theirs: not the discussion, not your replies, no mention of talking it over with you. A thread left out is ",
                            "posted as the reviewer's own messages."
                        ),
                    },
                }),
                &[],
            ),
        ),
        (
            "repo_map",
            concat!(
                "Outline of the repository: every source file with the classes, functions and methods it defines (Tree-sitter). ",
                "Built on demand, cached, and refreshed for changed files. Use it to find where something lives before reading or ",
                "searching files; narrow it with `path` (a folder or file) or `query` (a name or word)."
            )
            .to_string(),
            schema(
                json!({
                    "root": described(ROOT),
                    "path": described("Only files under this repository-relative folder (or this file)"),
                    "query": described("Only files whose path or symbol names contain this (case-insensitive)"),
                    "refresh": {
                        "type": "boolean",
                        "description": "Re-check every file now instead of reusing a map from the last few seconds",
                    },
                    "overview": {
                        "type": "boolean",
                        "description": concat!(
                            "Instead of the file list: the repository overview (where to start, the areas of ",
                            "the code and how they connect; from graphify-out/graph.json when the repository has a Graphify graph)"
                        ),
                    },
                }),
                &[],
            ),
        ),
        (
            "get_review",
            concat!(
                "Non-blocking snapshot: the latest verdict, open comments (the submitted batch), all threads with location and messages, ",
                "and `coverage` (repository reviews): source files the reviewer marked as viewed, overall and per area."
            )
            .to_string(),
            schema(json!({ "reviewId": described(REVIEW), "root": described(ROOT) }), &[]),
        ),
    ]
}

fn open_review_description() -> String {
    format!(
        "{}{}",
        concat!(
            "Open a review and join it as co-reviewer; tell the reviewer it is open in Co-Review. Either a repository (`root`, ",
            "default: the one Co-Review was started for) or a review directory / inline content: `dir` (index.markdown + assets + *.patch), ",
            "`markdown` and/or `patch` strings. A `dir` can also be a knowledge bundle such as OKF (`<repo>/okf`): every Markdown page ",
            "is a review page, `/x.md` links resolve from the bundle root and `path:line` links open the code. ",
            "To have the reviewer start from your findings, prepare it first: open_review({ ..., open: false }), add_findings, ",
            "then open_review({ reviewId }) shows it. ",
            "Then loop await_reviewer → reply until the reviewer submits.\n\n",
            "Design documents: to have a design reviewed before implementing, write the smallest document that lets the reviewer ",
            "understand, challenge and approve the change:\n"
        ),
        DESIGN_PROMPT
    )
}

fn open_review_properties() -> Value {
    json!({
        "reviewId": described(concat!(
            "An existing review to join and show (e.g. the one you prepared with open: false); ",
            "the other arguments except open and overview are then ignored"
        )),
        "root": described(ROOT),
        "dir": described("Review directory (index.markdown or *.pseudocode.md, *.patch, PR.md, <patch>.comments.json)"),
        "markdown": described("Inline Markdown document (```mermaid fences render as diagrams)"),
        "patch": described("Inline unified diff / git patch"),
        "diff": described(concat!(
            "Review a git diff of the repository (`root`), run by Co-Review: \"HEAD\" for uncommitted changes, ",
            "\"main...HEAD\" for a branch, any revision range. It becomes the patch page (named `patchName`, default \"change\"); ",
            "with `dir`, it is written into that directory (e.g. next to a PR.md)"
        )),
        "patchName": described("Slug for the inline patch page (default \"change\")"),
        "storePath": described("Where to write the review state file (default <dir>/review.json)"),
        "title": { "type": "string" },
        "open": {
            "type": "boolean",
            "description": concat!(
                "Show the review to the reviewer: the browser, or the desktop app's window (default true). ",
                "false prepares it without showing it (e.g. while you add first-pass findings); call again to show it"
            ),
        },
        "openspec": described("OpenSpec change directory (…/changes/<id>) shown with the document"),
        "overview": described(concat!(
            "Your overview for the review's front page (Markdown), which the reviewer reads first: ",
            "what the change (or repository) does and why, how it works as a ```mermaid flowchart, what else it affects (the blast ",
            "radius: callers, data, config, other services), and where to start. Written in the prepare step; pass it again ",
            "(with reviewId) to update it. Not for a design document, which is its own front page"
        )),
    })
}

fn finding_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": described("File path relative to the repository"),
            "line": { "type": "integer", "minimum": 1 },
            "endLine": { "type": "integer", "minimum": 1 },
            "body": described("Markdown"),
            "severity": { "type": "string", "enum": ["low", "medium", "high"] },
            "id": { "type": "string" },
            "target": { "type": "string" },
            "anchor": { "type": "object", "additionalProperties": {} },
            "labels": { "type": "array", "items": { "type": "string" } },
            "verdict": { "type": "string" },
            "status": {
                "type": "string",
                "enum": ["open", "proposed"],
                "description": "Code findings: \"proposed\" to have the reviewer triage them (default \"open\")",
            },
            "proposal": {
                "type": "object",
                "properties": {
                    "before": { "type": "string" },
                    "after": { "type": "string" },
                    "path": { "type": "string" },
                    "startLine": { "type": "number" },
                },
                "required": ["before", "after"],
                "additionalProperties": true,
            },
        },
        "required": ["body"],
        "additionalProperties": true,
    })
}

fn described(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn schema(properties: Value, required: &[&str]) -> Value {
    let mut schema = json!({ "type": "object", "properties": properties });
    if let Some(object) = schema.as_object_mut() {
        if !required.is_empty() {
            object.insert("required".to_string(), json!(required));
        }
        object.insert("additionalProperties".to_string(), json!(false));
        object.insert(
            "$schema".to_string(),
            json!("http://json-schema.org/draft-07/schema#"),
        );
    }
    schema
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompts::SERVER_INSTRUCTIONS;

    /// Design decision 11: there is no URL to give; the review is open in Co-Review.
    #[test]
    fn agents_are_told_the_review_is_open_in_co_review_not_given_a_url() {
        assert!(SERVER_INSTRUCTIONS.contains("tell the reviewer the review is open in Co-Review"));
        assert!(!SERVER_INSTRUCTIONS.contains("URL"));
        // Up to the design prompt, which is a copy of prompts/design.md.
        let description = open_review_description();
        let summary = description.split("\n\n").next().unwrap_or_default();
        assert!(summary.contains("tell the reviewer it is open in Co-Review"));
        assert!(!summary.contains("URL"));
    }
}
