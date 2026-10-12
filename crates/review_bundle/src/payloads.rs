//! The agent-facing shape of threads: short ids (`t<N>`), `target` (`doc` / `doc:<path>` /
//! `patch:<slug>`) and raw anchors, used by the MCP tools and the `review.json` state file.
//! Port of `extensions/review/src/node/review-payloads.ts`.

use std::path::{Component, Path, PathBuf};

use review_model::{
    CodeAnchor, CodeLocation, DocAnchor, LocationKind, ParticipantKind, PatchAnchor, Proposal,
    Review, ReviewDecision, ReviewThread, Severity, ThreadIntent, ThreadStatus, is_overview_page,
};
use review_store::path_to_file_uri;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::patch::DOCUMENT_NAMES;

/// `t<N>`: the short id agents use for a thread.
pub fn thread_ref(thread: &ReviewThread) -> String {
    format!("t{}", thread.number)
}

/// Where a thread is: `overview`, `doc` (the review directory's document), `doc:<path>` (another
/// Markdown page of the directory), `patch:<slug>`, or `code`.
pub fn target_of(thread: &ReviewThread, dir: Option<&Path>) -> String {
    let location = &thread.location;
    if location.kind == LocationKind::Document {
        if location.uri.as_deref().is_some_and(is_overview_page) {
            return "overview".to_string();
        }
        let relative = dir
            .zip(location.uri.as_deref().and_then(file_uri_to_path))
            .map(|(dir, file)| relative_path(dir, &file));
        return match relative {
            Some(relative)
                if !relative.is_empty()
                    && !relative.starts_with("..")
                    && !DOCUMENT_NAMES.contains(&relative.as_str()) =>
            {
                format!("doc:{relative}")
            }
            _ => "doc".to_string(),
        };
    }
    if location.kind == LocationKind::Patch
        && let Some(file) = location.uri.as_deref().and_then(file_uri_to_path)
    {
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        return format!("patch:{}", patch_slug(&name));
    }
    "code".to_string()
}

/// The raw anchor of a thread; code threads map onto `code-line` / `code-range` / `code-file`.
pub fn anchor_of(thread: &ReviewThread, root: Option<&Path>) -> Value {
    let location = &thread.location;
    if let Some(doc_anchor) = &location.doc_anchor {
        return serde_json::to_value(doc_anchor).unwrap_or_else(|_| Value::Object(Map::new()));
    }
    if let Some(patch_anchor) = &location.patch_anchor {
        return serde_json::to_value(patch_anchor).unwrap_or_else(|_| Value::Object(Map::new()));
    }
    let relative = root
        .zip(location.uri.as_deref().and_then(file_uri_to_path))
        .map(|(root, file)| relative_path(root, &file))
        .filter(|relative| !relative.is_empty());
    let mut anchor = Map::new();
    match (&location.range, relative) {
        (Some(range), Some(relative)) => {
            let start = range.start.line + 1;
            let end = range.end.line + 1;
            if start == end {
                anchor.insert("type".into(), "code-line".into());
                anchor.insert("path".into(), relative.into());
                anchor.insert("line".into(), start.into());
            } else {
                anchor.insert("type".into(), "code-range".into());
                anchor.insert("path".into(), relative.into());
                anchor.insert("startLine".into(), start.into());
                anchor.insert("endLine".into(), end.into());
            }
            anchor.insert("side".into(), "new".into());
            if let Some(code_anchor) = &location.anchor {
                anchor.insert("source".into(), code_anchor.text.clone().into());
            }
        }
        (None, Some(relative)) => {
            anchor.insert("type".into(), "code-file".into());
            anchor.insert("path".into(), relative.into());
        }
        _ => {
            anchor.insert("type".into(), "document".into());
        }
    }
    Value::Object(anchor)
}

/// The location of a `{ target, anchor }` pair inside a review directory.
pub fn location_from_anchor(
    dir: &Path,
    doc_file: Option<&Path>,
    target: Option<&str>,
    anchor: &Value,
) -> CodeLocation {
    let anchor_type = match anchor.get("type") {
        Some(Value::String(anchor_type)) => anchor_type.clone(),
        Some(other) => other.to_string(),
        None => "undefined".to_string(),
    };
    let string_field = |key: &str| anchor.get(key).and_then(Value::as_str);
    let target = target.map(str::to_string).unwrap_or_else(|| {
        if anchor_type.starts_with("code-") || anchor_type == "patch" {
            "patch".to_string()
        } else {
            "doc".to_string()
        }
    });

    if target.starts_with("patch") {
        let file = target
            .split(':')
            .nth(1)
            .filter(|slug| !slug.is_empty())
            .map(|slug| patch_file(dir, slug));
        return CodeLocation {
            kind: LocationKind::Patch,
            uri: file.map(|file| path_to_file_uri(&file.to_string_lossy())),
            range: None,
            symbol: None,
            anchor: Some(code_anchor(string_field("source").unwrap_or(""))),
            doc_anchor: None,
            patch_anchor: serde_json::from_value::<PatchAnchor>(anchor.clone()).ok(),
            tracked: None,
            extra: Map::new(),
        };
    }

    // `doc:<path>`: another page of the review directory; it must stay inside it.
    let directory = resolve_path(dir);
    let page = target
        .strip_prefix("doc:")
        .map(|page| resolve_path(&directory.join(page)))
        .filter(|page| {
            page.to_string_lossy().starts_with(&format!(
                "{}{}",
                directory.to_string_lossy(),
                std::path::MAIN_SEPARATOR
            ))
        });
    let file = page.or_else(|| doc_file.map(Path::to_path_buf));
    let text = string_field("exact")
        .or_else(|| string_field("source"))
        .unwrap_or("");
    CodeLocation {
        kind: LocationKind::Document,
        uri: file.map(|file| path_to_file_uri(&file.to_string_lossy())),
        range: None,
        symbol: None,
        anchor: Some(code_anchor(text)),
        doc_anchor: serde_json::from_value::<DocAnchor>(anchor.clone()).ok(),
        patch_anchor: None,
        tracked: None,
        extra: Map::new(),
    }
}

fn code_anchor(text: &str) -> CodeAnchor {
    CodeAnchor {
        text: text.to_string(),
        before: None,
        after: None,
        syntax: None,
    }
}

/// The patch file of a slug: `<slug>.patch` or `<slug>.diff`, whichever exists (else `.patch`).
pub fn patch_file(dir: &Path, slug: &str) -> PathBuf {
    [".patch", ".diff"]
        .iter()
        .map(|extension| dir.join(format!("{slug}{extension}")))
        .find(|file| file.exists())
        .unwrap_or_else(|| dir.join(format!("{slug}.patch")))
}

/// A patch file name without its `.patch` / `.diff` extension.
pub fn patch_slug(name: &str) -> &str {
    name.strip_suffix(".patch")
        .or_else(|| name.strip_suffix(".diff"))
        .unwrap_or(name)
}

/// The document of a review directory, as `export` writes it into `review.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateDocument {
    pub slug: String,
    pub title: String,
    pub markdown: String,
}

/// The review's state file (`review.json`), for tools that read the review from disk.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StateFile {
    pub doc: StateFileDocument,
    pub threads: Vec<StateThread>,
    pub seq: u32,
    pub review: StateReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StateFileDocument {
    pub slug: String,
    pub title: String,
    pub version: u32,
    pub markdown: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateThread {
    pub id: String,
    pub target: String,
    pub anchor: Value,
    pub status: ThreadStatus,
    pub severity: Severity,
    pub labels: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    pub intent: ThreadIntent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal: Option<Proposal>,
    pub messages: Vec<StateMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StateMessage {
    pub author: String,
    pub role: ParticipantKind,
    pub time: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateReview {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<ReviewDecision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submitted_at: Option<String>,
    pub count: u32,
}

pub fn to_state_file(review: &Review, document: Option<&StateDocument>) -> StateFile {
    let root = review
        .bundle
        .as_ref()
        .map(|bundle| PathBuf::from(&bundle.dir))
        .or_else(|| file_uri_to_path(&review.workspace_root));
    let root = root.as_deref();
    let threads = review
        .threads
        .iter()
        .map(|thread| StateThread {
            id: thread_ref(thread),
            target: target_of(thread, root),
            anchor: anchor_of(thread, root),
            status: thread.status,
            severity: thread.severity.unwrap_or(Severity::Medium),
            labels: thread.labels.clone().unwrap_or_default(),
            origin: thread.origin.clone(),
            intent: thread.intent,
            source_id: thread.source_id.clone(),
            proposal: thread.proposal.clone(),
            messages: thread
                .messages
                .iter()
                .filter(|message| message.status != Some(review_model::MessageStatus::Streaming))
                .map(|message| StateMessage {
                    author: message.author.name.clone(),
                    role: message.author.kind.clone(),
                    time: clock(message.created_at),
                    body: message.body.clone(),
                })
                .collect(),
        })
        .collect();
    let state_review = match &review.verdict {
        Some(verdict) => StateReview {
            decision: Some(verdict.decision),
            summary: Some(verdict.summary.clone()),
            submitted_at: Some(clock(verdict.submitted_at)),
            count: verdict.count,
        },
        None => StateReview {
            decision: None,
            summary: None,
            submitted_at: None,
            count: 0,
        },
    };
    StateFile {
        doc: StateFileDocument {
            slug: document.map(|doc| doc.slug.clone()).unwrap_or_default(),
            title: document
                .map(|doc| doc.title.clone())
                .unwrap_or_else(|| review.title.clone()),
            version: review
                .bundle
                .as_ref()
                .and_then(|bundle| bundle.doc_version)
                .unwrap_or(1),
            markdown: document.map(|doc| doc.markdown.clone()).unwrap_or_default(),
        },
        threads,
        seq: review.next_thread_number.saturating_sub(1),
        review: state_review,
    }
}

/// An accepted patch suggestion, for the agent to apply as a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedSuggestion {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    pub before: String,
    pub after: String,
}

pub fn accepted_suggestions(review: &Review) -> Vec<AcceptedSuggestion> {
    review
        .threads
        .iter()
        .filter(|thread| thread.location.kind == LocationKind::Patch)
        .filter_map(|thread| {
            let proposal = thread.proposal.as_ref()?;
            if proposal.status != review_model::ProposalStatus::Accepted {
                return None;
            }
            let patch_anchor = thread.location.patch_anchor.as_ref();
            Some(AcceptedSuggestion {
                thread_id: thread_ref(thread),
                path: proposal
                    .path
                    .clone()
                    .or_else(|| patch_anchor.and_then(|anchor| anchor.path.clone())),
                start_line: proposal
                    .start_line
                    .or_else(|| patch_anchor.and_then(|anchor| anchor.line.or(anchor.start_line))),
                before: proposal.before.clone(),
                after: proposal.after.clone(),
            })
        })
        .collect()
}

/// `HH:MM` in local time, as the TypeScript app writes message times.
fn clock(milliseconds: u64) -> String {
    use chrono::TimeZone as _;
    i64::try_from(milliseconds)
        .ok()
        .and_then(|milliseconds| chrono::Local.timestamp_millis_opt(milliseconds).single())
        .map(|time| time.format("%H:%M").to_string())
        .unwrap_or_else(|| "00:00".to_string())
}

/// The file system path of a `file:` URI (percent-decoded), as Theia's `FileUri.fsPath`.
pub fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                decoded.push(byte);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    Some(PathBuf::from(
        String::from_utf8_lossy(&decoded).into_owned(),
    ))
}

/// `path.resolve`: absolute against the current directory, `.` and `..` resolved lexically.
pub fn resolve_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other),
        }
    }
    resolved
}

/// `path.relative(from, to)` with `/` separators: `""` for the same path, `../…` outside `from`.
pub fn relative_path(from: &Path, to: &Path) -> String {
    let from = resolve_path(from);
    let to = resolve_path(to);
    let from_parts: Vec<Component> = from.components().collect();
    let to_parts: Vec<Component> = to.components().collect();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> = from_parts[common..]
        .iter()
        .map(|_| "..".to_string())
        .collect();
    parts.extend(
        to_parts[common..]
            .iter()
            .map(|part| part.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_match_node() {
        assert_eq!(
            relative_path(Path::new("/a/b"), Path::new("/a/b/c/d.md")),
            "c/d.md"
        );
        assert_eq!(
            relative_path(Path::new("/a/b"), Path::new("/a/x.md")),
            "../x.md"
        );
        assert_eq!(relative_path(Path::new("/a/b"), Path::new("/a/b")), "");
        assert_eq!(
            resolve_path(Path::new("/a/b/../c/./d/")),
            PathBuf::from("/a/c/d")
        );
    }

    #[test]
    fn file_uris_decode_utf8() {
        assert_eq!(
            file_uri_to_path("file:///tmp/r%C3%A9sum%C3%A9%20x.md"),
            Some(PathBuf::from("/tmp/résumé x.md"))
        );
        assert_eq!(file_uri_to_path("untitled:x"), None);
    }

    #[test]
    fn patch_targets_without_a_file_still_name_the_patch() {
        let location = location_from_anchor(
            Path::new("/nowhere"),
            None,
            Some("patch:change"),
            &serde_json::json!({ "type": "patch" }),
        );
        assert_eq!(
            location.uri.as_deref(),
            Some("file:///nowhere/change.patch")
        );
    }
}
