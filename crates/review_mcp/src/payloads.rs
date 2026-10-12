//! The agent-facing shape of threads: the `comments[]` entries and the `Thread` objects `await_reviewer` and
//! `get_review` return (the payload parts of `co-reviewer-mcp.ts`), over `review_bundle::payloads` (short ids,
//! targets, raw anchors).
//!
//! Objects are built key by key in the TypeScript's order, leaving out what it leaves `undefined`, so the JSON text
//! matches.

use std::path::Path;

use review_model::{AnchorStatus, LocationKind, Review, ReviewThread, parse_thread_short_id};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::paths::{relative, uri_to_path};

pub use review_bundle::payloads::thread_ref;

/// Builds a JSON object in insertion order, skipping absent values as `JSON.stringify` skips `undefined`.
#[derive(Default)]
pub struct Object(Map<String, Value>);

impl Object {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(mut self, key: &str, value: impl Serialize) -> Self {
        self.0.insert(key.to_string(), to_value(value));
        self
    }

    pub fn maybe(mut self, key: &str, value: Option<impl Serialize>) -> Self {
        if let Some(value) = value {
            self.0.insert(key.to_string(), to_value(value));
        }
        self
    }

    /// Adds every key of `other` (the TypeScript's `...other`).
    pub fn spread(mut self, other: Map<String, Value>) -> Self {
        for (key, value) in other {
            self.0.insert(key, value);
        }
        self
    }

    pub fn into_map(self) -> Map<String, Value> {
        self.0
    }

    pub fn build(self) -> Value {
        Value::Object(self.0)
    }
}

fn to_value(value: impl Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// A thread by its full id or its short id (`t<N>`).
pub fn find_thread<'a>(review: &'a Review, id: &str) -> Option<&'a ReviewThread> {
    let number = id
        .strip_prefix('t')
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|_| parse_thread_short_id(id));
    review
        .threads
        .iter()
        .find(|thread| thread.id == id || number.is_some_and(|number| thread.number == number))
}

/// Where a review's pages are: its review directory, else its repository.
pub fn root_of(review: &Review) -> String {
    match &review.bundle {
        Some(bundle) => bundle.dir.clone(),
        None => uri_to_path(&review.workspace_root),
    }
}

/// Where a review's code is: a change review's code folder, else its repository (or review directory).
pub fn code_root_of(review: &Review) -> String {
    review
        .bundle
        .as_ref()
        .and_then(|bundle| bundle.code.clone())
        .unwrap_or_else(|| uri_to_path(&review.workspace_root))
}

/// The folder a review's window shows: its code when it has some, else its review directory or repository.
pub fn window_root_of(review: &Review) -> String {
    review
        .bundle
        .as_ref()
        .and_then(|bundle| bundle.code.clone())
        .unwrap_or_else(|| root_of(review))
}

/// Where a thread is: `doc`, `doc:<path>` (another Markdown page of the directory), `patch:<slug>`, `overview` or
/// `code`.
pub fn target_of(thread: &ReviewThread, dir: &str) -> String {
    review_bundle::payloads::target_of(thread, Some(Path::new(dir)))
}

/// Accepted suggested edits on patches, for the agent to apply as commits.
pub fn accepted_suggestions(review: &Review) -> Value {
    to_value(review_bundle::payloads::accepted_suggestions(review))
}

/// One entry of `comments[]` (await_reviewer, get_review); code paths relative to `code_root`.
pub fn comment_of(
    thread: &ReviewThread,
    root: &str,
    extra: Map<String, Value>,
    code_root: &str,
) -> Value {
    let anchor = match review_bundle::payloads::anchor_of(thread, Some(Path::new(code_root))) {
        Value::Object(anchor) => anchor,
        _ => Map::new(),
    };
    let text = |key: &str| anchor.get(key).filter(|value| !value.is_null()).cloned();
    let place = text("path")
        .or_else(|| text("blockId"))
        .or_else(|| text("nodeId"))
        .unwrap_or_else(|| json!("document"));
    let line = anchor
        .get("line")
        .filter(|value| value.is_number())
        .or_else(|| anchor.get("startLine").filter(|value| value.is_number()))
        .cloned();
    Object::new()
        .set("id", thread_ref(thread))
        .set("threadId", &thread.id)
        .set("target", target_of(thread, root))
        .maybe("kind", text("type"))
        .set("where", place)
        .maybe("line", line)
        .maybe("side", text("side"))
        .maybe("severity", thread.severity)
        .set("labels", thread.labels.clone().unwrap_or_default())
        .set("status", thread.status)
        .set(
            "body",
            thread
                .messages
                .last()
                .map(|message| message.body.as_str())
                .unwrap_or(""),
        )
        .maybe("source", text("source").or_else(|| text("exact")))
        .maybe("origin", thread.origin.as_ref())
        .set("intent", thread.intent)
        .maybe(
            "suggestion",
            thread.proposal.as_ref().map(|proposal| &proposal.after),
        )
        .spread(extra)
        .build()
}

/// A thread for the agent: where it is, the code, what happened to that code, and the conversation.
pub fn describe(review: &Review, thread: &ReviewThread) -> Value {
    let location = &thread.location;
    // Pages of a review directory relative to it; code relative to the code folder.
    let root = if matches!(location.kind, LocationKind::Document | LocationKind::Patch) {
        root_of(review)
    } else {
        code_root_of(review)
    };
    let patch = location.patch_anchor.as_ref();
    let path = patch.and_then(|anchor| anchor.path.clone()).or_else(|| {
        location
            .uri
            .as_ref()
            .map(|uri| relative(&root, &uri_to_path(uri)))
    });
    let start_line = match &location.range {
        Some(range) => Some(range.start.line + 1),
        None => patch.and_then(|anchor| anchor.line.or(anchor.start_line)),
    };
    let end_line = match &location.range {
        Some(range) => Some(range.end.line + 1),
        None => patch.and_then(|anchor| anchor.line.or(anchor.end_line)),
    };
    let anchor = location
        .doc_anchor
        .as_ref()
        .map(to_value)
        .or_else(|| patch.map(to_value));
    let code = (location.kind != LocationKind::Repository)
        .then(|| location.anchor.as_ref().map(|anchor| anchor.text.clone()))
        .flatten();
    let mut tracked = Object::new();
    if let Some(range) = &location.tracked {
        tracked = tracked.set("anchorStatus", range.status);
        if range.status == AnchorStatus::Modified {
            tracked = tracked.set("original", &range.original);
        }
        if range.status == AnchorStatus::Ambiguous {
            tracked = tracked.maybe("candidates", range.candidates.as_ref().map(Vec::len));
        }
    }
    let messages: Vec<Value> = thread
        .messages
        .iter()
        .filter(|message| !message.body.trim().is_empty())
        .map(|message| {
            Object::new()
                .set("author", &message.author.name)
                .set("role", &message.author.kind)
                .set("body", &message.body)
                .build()
        })
        .collect();
    Object::new()
        .set("threadId", &thread.id)
        .set("id", thread_ref(thread))
        .set("number", thread.number)
        .set("status", thread.status)
        .set("intent", thread.intent)
        .maybe("severity", thread.severity)
        .maybe("labels", thread.labels.as_ref())
        .maybe("anchor", anchor)
        .set(
            "location",
            Object::new()
                .set("kind", location.kind)
                .maybe("path", path)
                .maybe("startLine", start_line)
                .maybe("endLine", end_line)
                .maybe("symbol", location.symbol.as_ref())
                .build(),
        )
        .maybe("code", code)
        .spread(tracked.into_map())
        .set("messages", messages)
        .build()
}
