//! Data types for Co-Review's review model, compatible with the TypeScript app's JSON.
//!
//! Every struct uses `#[serde(rename_all = "camelCase")]` so field names match the TypeScript
//! convention. Main structs carry `#[serde(flatten)] extra` to preserve unknown fields on round
//! trip, so newer or older data is never lost.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Position and Range (LSP-style, zero-based)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

// ---------------------------------------------------------------------------
// Anchor status and TrackedRange (from anchor-tracking.ts — type only)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnchorStatus {
    Active,
    Modified,
    Removed,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedRangeCandidate {
    pub from: u64,
    pub to: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedRange {
    pub version: String,
    pub from: u64,
    pub to: u64,
    pub text: String,
    pub prefix: String,
    pub suffix: String,
    pub status: AnchorStatus,
    pub original: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidates: Option<Vec<TrackedRangeCandidate>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Anchors: Code, Syntax, Doc, Patch
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyntaxAnchor {
    pub grammar: String,
    pub tokens: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeAnchor {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub syntax: Option<SyntaxAnchor>,
}

/// The `type` of a document anchor. Anchors are stored verbatim (whatever a finding carried), so a
/// missing or unknown `type` deserializes to `Other` and its original value stays in `extra`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocAnchorType {
    #[serde(rename = "document")]
    Document,
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "mermaid-block")]
    MermaidBlock,
    #[serde(rename = "mermaid-node")]
    MermaidNode,
    #[serde(rename = "mermaid-edge")]
    MermaidEdge,
    #[serde(rename = "tree-node")]
    TreeNode,
    #[serde(skip)]
    Other,
}

impl DocAnchorType {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "document" => Some(Self::Document),
            "text" => Some(Self::Text),
            "mermaid-block" => Some(Self::MermaidBlock),
            "mermaid-node" => Some(Self::MermaidNode),
            "mermaid-edge" => Some(Self::MermaidEdge),
            "tree-node" => Some(Self::TreeNode),
            _ => None,
        }
    }

    fn is_other(&self) -> bool {
        *self == Self::Other
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocAnchor {
    #[serde(rename = "type", skip_serializing_if = "DocAnchorType::is_other")]
    pub anchor_type: DocAnchorType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exact: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_offset_hint: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_offset_hint: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurrence: Option<u32>,
    /// Every field that is not one of the above or does not have its type (e.g. `"line": "12"`),
    /// kept as it was so the anchor round-trips exactly.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl<'de> Deserialize<'de> for DocAnchor {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = serde_json::Map::deserialize(deserializer)?;
        Ok(Self {
            anchor_type: take_anchor_type(&mut fields, DocAnchorType::from_name)
                .unwrap_or(DocAnchorType::Other),
            exact: take_field(&mut fields, "exact"),
            prefix: take_field(&mut fields, "prefix"),
            suffix: take_field(&mut fields, "suffix"),
            start_offset_hint: take_field(&mut fields, "startOffsetHint"),
            end_offset_hint: take_field(&mut fields, "endOffsetHint"),
            block_id: take_field(&mut fields, "blockId"),
            node_id: take_field(&mut fields, "nodeId"),
            edge_id: take_field(&mut fields, "edgeId"),
            source: take_field(&mut fields, "source"),
            occurrence: take_field(&mut fields, "occurrence"),
            extra: fields,
        })
    }
}

/// Removes `key` from `fields` when it holds a `T`; anything else (`null` too) stays for `extra`.
fn take_field<T: serde::de::DeserializeOwned>(
    fields: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<T> {
    let value = T::deserialize(fields.get(key)?).ok()?;
    fields.shift_remove(key);
    Some(value)
}

fn take_anchor_type<T>(
    fields: &mut serde_json::Map<String, serde_json::Value>,
    from_name: fn(&str) -> Option<T>,
) -> Option<T> {
    let anchor_type = from_name(fields.get("type")?.as_str()?)?;
    fields.shift_remove("type");
    Some(anchor_type)
}

/// Lengths and prefixes in UTF-16 code units, as JavaScript's `length` and `slice` count them.
fn utf16_length(text: &str) -> usize {
    text.encode_utf16().count()
}

fn utf16_prefix(text: &str, units: usize) -> String {
    let code_units: Vec<u16> = text.encode_utf16().take(units).collect();
    String::from_utf16_lossy(&code_units)
}

impl DocAnchor {
    pub fn describe(&self) -> String {
        match self.anchor_type {
            DocAnchorType::Document => "document".to_string(),
            DocAnchorType::Text => {
                let exact = self.exact.as_deref().unwrap_or("");
                let ellipsis = if utf16_length(exact) > 60 { "\u{2026}" } else { "" };
                format!("\u{201c}{}{ellipsis}\u{201d}", utf16_prefix(exact, 60))
            }
            DocAnchorType::MermaidBlock => {
                format!("diagram {}", self.block_id.as_deref().unwrap_or(""))
            }
            DocAnchorType::MermaidNode => {
                format!(
                    "diagram {} \u{203a} {}",
                    self.block_id.as_deref().unwrap_or(""),
                    self.node_id.as_deref().unwrap_or("")
                )
            }
            DocAnchorType::MermaidEdge => {
                format!(
                    "diagram {} \u{203a} {}",
                    self.block_id.as_deref().unwrap_or(""),
                    self.edge_id.as_deref().unwrap_or("")
                )
            }
            DocAnchorType::TreeNode => {
                let node_id = self.node_id.as_deref().unwrap_or("");
                format!("step \u{201c}{}\u{201d}", utf16_prefix(node_id, 50))
            }
            // The TypeScript `describe` returns `undefined` here, which its callers interpolate.
            DocAnchorType::Other => "undefined".to_string(),
        }
    }
}

/// The `type` of a patch anchor; a missing or unknown `type` (e.g. a `text` anchor a finding put
/// on a patch) deserializes to `Other` and its original value stays in `extra`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PatchAnchorType {
    #[serde(rename = "patch")]
    Patch,
    #[serde(rename = "code-file")]
    CodeFile,
    #[serde(rename = "code-line")]
    CodeLine,
    #[serde(rename = "code-range")]
    CodeRange,
    #[serde(skip)]
    Other,
}

impl PatchAnchorType {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "patch" => Some(Self::Patch),
            "code-file" => Some(Self::CodeFile),
            "code-line" => Some(Self::CodeLine),
            "code-range" => Some(Self::CodeRange),
            _ => None,
        }
    }

    fn is_other(&self) -> bool {
        *self == Self::Other
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PatchSide {
    New,
    Old,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchAnchor {
    #[serde(rename = "type", skip_serializing_if = "PatchAnchorType::is_other")]
    pub anchor_type: PatchAnchorType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<PatchSide>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Every field that is not one of the above or does not have its type, kept as it was.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl<'de> Deserialize<'de> for PatchAnchor {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = serde_json::Map::deserialize(deserializer)?;
        Ok(Self {
            anchor_type: take_anchor_type(&mut fields, PatchAnchorType::from_name)
                .unwrap_or(PatchAnchorType::Other),
            path: take_field(&mut fields, "path"),
            line: take_field(&mut fields, "line"),
            start_line: take_field(&mut fields, "startLine"),
            end_line: take_field(&mut fields, "endLine"),
            side: take_field(&mut fields, "side"),
            source: take_field(&mut fields, "source"),
            extra: fields,
        })
    }
}

impl PatchAnchor {
    pub fn describe(&self) -> String {
        match self.anchor_type {
            PatchAnchorType::Patch => "patch".to_string(),
            PatchAnchorType::CodeFile => {
                self.path.as_deref().unwrap_or("file").to_string()
            }
            PatchAnchorType::CodeLine => {
                let path = self.path.as_deref().unwrap_or("");
                let line = self.line.unwrap_or(0);
                let side_suffix = if self.side == Some(PatchSide::Old) {
                    " (old)"
                } else {
                    ""
                };
                format!("{path}:{line}{side_suffix}")
            }
            PatchAnchorType::CodeRange => {
                let path = self.path.as_deref().unwrap_or("");
                let start = self.start_line.unwrap_or(0);
                let end = self.end_line.unwrap_or(0);
                let side_suffix = if self.side == Some(PatchSide::Old) {
                    " (old)"
                } else {
                    ""
                };
                format!("{path}:{start}-{end}{side_suffix}")
            }
            PatchAnchorType::Other => "undefined".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Location
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocationKind {
    Repository,
    Directory,
    File,
    Symbol,
    Line,
    Range,
    Document,
    Patch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeLocation {
    pub kind: LocationKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<CodeAnchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_anchor: Option<DocAnchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_anchor: Option<PatchAnchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracked: Option<TrackedRange>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Participants
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParticipantKind {
    Human,
    Agent,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Participant {
    pub id: String,
    pub kind: ParticipantKind,
    pub name: String,
}

// ---------------------------------------------------------------------------
// Agent activity, permissions, messages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentActivity {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub status: ToolStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionOption {
    pub id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub id: String,
    pub title: String,
    pub options: Vec<PermissionOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageStatus {
    Streaming,
    Done,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewMessage {
    pub id: String,
    pub author: Participant,
    pub body: String,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<MessageStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<Vec<AgentActivity>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<PermissionRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Agent config and settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentTransport {
    Acp,
    Mcp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<AgentTransport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_label: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl AgentConfig {
    pub fn is_acp(&self) -> bool {
        matches!(self.transport, None | Some(AgentTransport::Acp)) && self.command.is_some()
    }

    pub fn command_line(&self) -> String {
        let mut parts = Vec::new();
        if let Some(cmd) = &self.command {
            parts.push(cmd.clone());
        }
        if let Some(args) = &self.args {
            parts.extend(args.iter().cloned());
        }
        parts.join(" ").trim().to_string()
    }

    pub fn same_process(&self, other: &AgentConfig) -> bool {
        self.id == other.id
            && self.transport == other.transport
            && self.command_line() == other.command_line()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSettingOption {
    pub value: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSetting {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub current: String,
    pub options: Vec<AgentSettingOption>,
}

impl AgentSetting {
    pub const MODE: &'static str = "__mode";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresence {
    pub review_id: String,
    pub listening: bool,
    pub last_seen: u64,
}

// ---------------------------------------------------------------------------
// Agent state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Idle,
    Queued,
    Working,
    WaitingForHuman,
}

// ---------------------------------------------------------------------------
// Threads
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadStatus {
    Open,
    Resolved,
    Proposed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadIntent {
    Comment,
    Question,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalStatus {
    Pending,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub before: String,
    pub after: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    pub status: ProposalStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<ThreadIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ThreadStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<Proposal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewThread {
    pub id: String,
    pub number: u32,
    pub location: CodeLocation,
    pub messages: Vec<ReviewMessage>,
    pub status: ThreadStatus,
    pub intent: ThreadIntent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<Proposal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_state: Option<AgentState>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Review scope
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ReviewScope {
    #[serde(rename = "repository")]
    Repository,
    #[serde(rename = "paths")]
    Paths { uris: Vec<String> },
    #[serde(rename = "branch")]
    Branch { base: String, head: String },
    #[serde(rename = "commit")]
    Commit { sha: String },
}

impl ReviewScope {
    pub fn label(&self) -> String {
        self.label_with(|u| u.to_string())
    }

    pub fn label_with(&self, relative: impl Fn(&str) -> String) -> String {
        match self {
            ReviewScope::Repository => "Entire repository".to_string(),
            ReviewScope::Paths { uris } => {
                uris.iter().map(|u| relative(u)).collect::<Vec<_>>().join(", ")
            }
            ReviewScope::Branch { base, head } => format!("{base}...{head}"),
            ReviewScope::Commit { sha } => format!("Commit {}", &sha[..sha.len().min(10)]),
        }
    }
}

// ---------------------------------------------------------------------------
// Activity log
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActivityKind {
    ReviewCreated,
    ThreadCreated,
    MessageAdded,
    ThreadResolved,
    ThreadReopened,
    ReviewSubmitted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewActivity {
    pub kind: ActivityKind,
    pub actor: Participant,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub at: u64,
}

// ---------------------------------------------------------------------------
// Review bundle, verdict, closed
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckoutKind {
    Neighbourhood,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleGitHub {
    pub url: String,
    pub round: u32,
    pub posted_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewBundle {
    pub dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openspec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<BundleGitHub>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout: Option<CheckoutKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewDecision {
    Approve,
    RequestChanges,
    Comment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerdictToAgent {
    Waiting,
    Delivered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewVerdict {
    pub decision: ReviewDecision,
    pub summary: String,
    pub submitted_at: u64,
    pub count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_agent: Option<VerdictToAgent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewClosed {
    pub at: u64,
}

// ---------------------------------------------------------------------------
// Review (top-level)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewed: Option<HashMap<String, u64>>,
    pub workspace_root: String,
    pub scope: ReviewScope,
    pub participants: Vec<Participant>,
    pub threads: Vec<ReviewThread>,
    pub activity: Vec<ReviewActivity>,
    pub next_thread_number: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<ReviewBundle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<ReviewVerdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed: Option<ReviewClosed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<u64>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Review coverage (not stored, computed at runtime)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewCoverageArea {
    pub path: String,
    pub total: u32,
    pub viewed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewCoverage {
    pub total: u32,
    pub viewed: u32,
    pub areas: Vec<ReviewCoverageArea>,
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// FNV-1a content version hash, matching the TypeScript implementation exactly.
/// Iterates UTF-16 code units (JavaScript semantics), produces `"<hex8>-<len_base36>"`.
pub fn content_version(text: &str) -> String {
    let mut hash: u32 = 0x811c9dc5;
    let mut utf16_len: usize = 0;
    for unit in text.encode_utf16() {
        hash ^= unit as u32;
        hash = hash.wrapping_mul(0x01000193);
        utf16_len += 1;
    }
    let hex = format!("{hash:08x}");
    let len_b36 = radix_36(utf16_len);
    format!("{hex}-{len_b36}")
}

fn radix_36(mut n: usize) -> String {
    if n == 0 {
        return "0".to_string();
    }
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut buf = Vec::new();
    while n > 0 {
        buf.push(DIGITS[n % 36]);
        n /= 36;
    }
    buf.reverse();
    String::from_utf8(buf).expect("radix_36 produces ASCII")
}

/// Whether a URI points to a review's overview page (or the legacy findings page).
pub fn is_overview_page(uri: &str) -> bool {
    // Match: /pages/<segment>/overview.md or /findings/<segment>/findings.md
    // where <segment> contains no '/'.
    if let Some(before_overview) = uri.strip_suffix("/overview.md")
        && let Some(pos) = before_overview.rfind("/pages/")
    {
        let segment = &before_overview[pos + "/pages/".len()..];
        if !segment.contains('/') {
            return true;
        }
    }
    if let Some(before_findings) = uri.strip_suffix("/findings.md")
        && let Some(pos) = before_findings.rfind("/findings/")
    {
        let segment = &before_findings[pos + "/findings/".len()..];
        if !segment.contains('/') {
            return true;
        }
    }
    false
}

/// Whether a thread's target was removed or is ambiguous: shown in the panel only, "out of scope".
pub fn is_out_of_scope(thread: &ReviewThread) -> bool {
    if thread.status == ThreadStatus::Resolved {
        return false;
    }
    if let Some(tracked) = &thread.location.tracked {
        return tracked.status == AnchorStatus::Removed || tracked.status == AnchorStatus::Ambiguous;
    }
    false
}

/// Whether the reviewer's latest message in a thread is waiting for the given MCP agent.
pub fn waits_for_agent(review: &Review, thread: &ReviewThread, agent_id: &str) -> bool {
    let last = match thread.messages.last() {
        Some(m) => m,
        None => return false,
    };
    if thread.status == ThreadStatus::Resolved || last.author.kind != ParticipantKind::Human {
        return false;
    }
    let seated = review
        .agent
        .as_ref()
        .is_some_and(|a| a.transport == Some(AgentTransport::Mcp) && a.id == agent_id);
    ((thread.intent == ThreadIntent::Question || review.bundle.is_some()) && seated)
        || thread.messages.iter().any(|m| m.author.id == agent_id)
}

/// The short id for a thread number: `t1`, `t2`, etc.
pub fn thread_short_id(number: u32) -> String {
    format!("t{number}")
}

/// Parse a thread short id like `t5` into its number.
pub fn parse_thread_short_id(s: &str) -> Option<u32> {
    s.strip_prefix('t').and_then(|rest| rest.parse().ok())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_content_version_ascii() {
        // Verified against the TypeScript implementation
        assert_eq!(content_version(""), "811c9dc5-0");
        assert_eq!(content_version("hello"), "4f9f2cab-5");
    }

    #[test]
    fn test_content_version_non_bmp() {
        // U+1F600 (😀) is 2 UTF-16 code units: 0xD83D 0xDE00
        // Length in UTF-16 = 2, which is "2" in base 36
        let result = content_version("\u{1F600}");
        // The hash must iterate over the two surrogates, not one u32 codepoint
        assert!(result.ends_with("-2"), "expected UTF-16 length 2, got {result}");
    }

    #[test]
    fn test_content_version_mixed() {
        // "a😀b" = 4 UTF-16 code units (a, D83D, DE00, b)
        let result = content_version("a\u{1F600}b");
        assert!(result.ends_with("-4"), "expected UTF-16 length 4, got {result}");
    }

    #[test]
    fn test_is_overview_page() {
        assert!(is_overview_page("/home/user/.co-review/pages/abc123/overview.md"));
        assert!(is_overview_page("file:///home/.co-review/findings/abc/findings.md"));
        assert!(!is_overview_page("/home/user/.co-review/pages/overview.md"));
        assert!(!is_overview_page("/home/user/random/overview.md"));
        assert!(!is_overview_page(""));
    }

    #[test]
    fn test_thread_short_id() {
        assert_eq!(thread_short_id(1), "t1");
        assert_eq!(thread_short_id(42), "t42");
        assert_eq!(parse_thread_short_id("t5"), Some(5));
        assert_eq!(parse_thread_short_id("t0"), Some(0));
        assert_eq!(parse_thread_short_id("abc"), None);
        assert_eq!(parse_thread_short_id("t"), None);
        assert_eq!(parse_thread_short_id("t-1"), None);
    }

    #[test]
    fn test_review_scope_label() {
        assert_eq!(ReviewScope::Repository.label(), "Entire repository");
        assert_eq!(
            ReviewScope::Paths { uris: vec!["file:///a".into(), "file:///b".into()] }.label(),
            "file:///a, file:///b"
        );
        assert_eq!(
            ReviewScope::Branch { base: "main".into(), head: "feature".into() }.label(),
            "main...feature"
        );
        assert_eq!(
            ReviewScope::Commit { sha: "abcdef1234567890".into() }.label(),
            "Commit abcdef1234"
        );
    }

    #[test]
    fn test_doc_anchor_describe() {
        let doc = DocAnchor {
            anchor_type: DocAnchorType::Text,
            exact: Some("short".into()),
            prefix: None, suffix: None,
            start_offset_hint: None, end_offset_hint: None,
            block_id: None, node_id: None, edge_id: None,
            source: None, occurrence: None,
            extra: serde_json::Map::new(),
        };
        assert_eq!(doc.describe(), "\u{201c}short\u{201d}");
        // A surrogate pair cut at the limit becomes U+FFFD; JavaScript keeps the lone surrogate.
        let long = DocAnchor { exact: Some(format!("{}\u{1f600}abc", "x".repeat(59))), ..doc.clone() };
        assert_eq!(long.describe(), format!("\u{201c}{}\u{fffd}\u{2026}\u{201d}", "x".repeat(59)));
        let step = DocAnchor { anchor_type: DocAnchorType::TreeNode, exact: None, node_id: Some("a".repeat(55)), ..doc };
        assert_eq!(step.describe(), format!("step \u{201c}{}\u{201d}", "a".repeat(50)));
    }

    #[test]
    fn test_patch_anchor_describe() {
        let pa = PatchAnchor {
            anchor_type: PatchAnchorType::CodeLine,
            path: Some("src/main.rs".into()),
            line: Some(42),
            start_line: None, end_line: None,
            side: Some(PatchSide::Old),
            source: None,
            extra: serde_json::Map::new(),
        };
        assert_eq!(pa.describe(), "src/main.rs:42 (old)");
    }

    #[test]
    fn test_anchors_round_trip_whatever_they_carry() {
        let doc_anchors = [
            r#"{"type":"text","exact":"a","prefix":"b","occurrence":2}"#,
            r#"{"exact":"no type"}"#,
            r#"{"type":"code-line","path":"a.rs","line":3}"#,
            r#"{"type":7,"blockId":["not","a","string"],"exact":null}"#,
        ];
        for json in doc_anchors {
            let anchor: DocAnchor = serde_json::from_str(json).unwrap();
            assert_eq!(serde_json::to_string(&anchor).unwrap(), json);
        }
        let patch_anchors = [
            r#"{"type":"code-range","path":"a.rs","startLine":1,"endLine":2,"side":"old"}"#,
            r#"{"type":"text","exact":"on a patch"}"#,
            r#"{"path":"a.rs","line":-1}"#,
            r#"{"type":"code-line","line":"12","side":"both"}"#,
        ];
        for json in patch_anchors {
            let anchor: PatchAnchor = serde_json::from_str(json).unwrap();
            assert_eq!(serde_json::to_string(&anchor).unwrap(), json);
        }

        let text_on_patch: PatchAnchor = serde_json::from_str(patch_anchors[1]).unwrap();
        assert_eq!(text_on_patch.anchor_type, PatchAnchorType::Other);
        assert_eq!(text_on_patch.describe(), "undefined");
        let typed: PatchAnchor = serde_json::from_str(patch_anchors[0]).unwrap();
        assert_eq!(typed.anchor_type, PatchAnchorType::CodeRange);
        assert_eq!(typed.side, Some(PatchSide::Old));
        assert!(typed.extra.is_empty());

        let location = serde_json::json!({
            "kind": "patch",
            "uri": "file:///r/change.patch",
            "anchor": { "text": "" },
            "patchAnchor": { "type": "text", "exact": "x" },
            "docAnchor": { "nodeId": "n" },
        });
        let parsed: CodeLocation = serde_json::from_value(location.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), location);
    }

    #[test]
    fn test_review_round_trip_synthetic() {
        let json = include_str!("../tests/fixtures/synthetic_review.json");
        let value_before: serde_json::Value = serde_json::from_str(json).unwrap();
        let review: Review = serde_json::from_value(value_before.clone()).unwrap();
        let value_after = serde_json::to_value(&review).unwrap();
        assert_eq!(value_before, value_after);
    }

    #[test]
    fn test_round_trip_real_data() {
        if std::env::var("CO_REVIEW_REAL_DATA").is_err() {
            eprintln!("Skipping real-data round trip (set CO_REVIEW_REAL_DATA=1 to run)");
            return;
        }
        let home = std::env::var("CO_REVIEW_HOME")
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap();
                format!("{home}/.co-review")
            });
        let workspaces_dir = format!("{home}/workspaces");
        let mut tested = 0;
        let mut failed = 0;
        for entry in find_review_files(&workspaces_dir) {
            let content = match std::fs::read_to_string(&entry) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("SKIP {entry}: {e}");
                    continue;
                }
            };
            let value_before: serde_json::Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("SKIP {entry}: parse error: {e}");
                    continue;
                }
            };
            let review: Review = match serde_json::from_value(value_before.clone()) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("FAIL {entry}: deserialize to Review: {e}");
                    failed += 1;
                    continue;
                }
            };
            let value_after = serde_json::to_value(&review).unwrap();
            if value_before != value_after {
                eprintln!("FAIL {entry}: round-trip mismatch");
                let diff = json_diff(&value_before, &value_after);
                for d in &diff[..diff.len().min(10)] {
                    eprintln!("  {d}");
                }
                failed += 1;
            }
            tested += 1;
        }
        eprintln!("Real-data round trip: {tested} files tested, {failed} failed");
        assert_eq!(failed, 0, "{failed} files failed round-trip");
        assert!(tested > 0, "No real review files found");
    }

    fn find_review_files(workspaces_dir: &str) -> Vec<String> {
        let mut results = Vec::new();
        let Ok(ws_entries) = std::fs::read_dir(workspaces_dir) else {
            return results;
        };
        for ws_entry in ws_entries.flatten() {
            let reviews_dir = ws_entry.path().join("reviews");
            let Ok(review_entries) = std::fs::read_dir(&reviews_dir) else {
                continue;
            };
            for review_entry in review_entries.flatten() {
                let path = review_entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json") {
                    results.push(path.to_string_lossy().to_string());
                }
            }
        }
        results
    }

    fn json_diff(a: &serde_json::Value, b: &serde_json::Value) -> Vec<String> {
        let mut diffs = Vec::new();
        json_diff_inner(a, b, "", &mut diffs);
        diffs
    }

    fn json_diff_inner(a: &serde_json::Value, b: &serde_json::Value, path: &str, out: &mut Vec<String>) {
        if a == b {
            return;
        }
        match (a, b) {
            (serde_json::Value::Object(ma), serde_json::Value::Object(mb)) => {
                for (k, v) in ma {
                    let p = format!("{path}.{k}");
                    match mb.get(k) {
                        Some(bv) => json_diff_inner(v, bv, &p, out),
                        None => out.push(format!("missing in re-serialized: {p}")),
                    }
                }
                for k in mb.keys() {
                    if !ma.contains_key(k) {
                        out.push(format!("extra in re-serialized: {path}.{k}"));
                    }
                }
            }
            (serde_json::Value::Array(va), serde_json::Value::Array(vb)) => {
                for (i, (ea, eb)) in va.iter().zip(vb.iter()).enumerate() {
                    json_diff_inner(ea, eb, &format!("{path}[{i}]"), out);
                }
                if va.len() != vb.len() {
                    out.push(format!("{path}: array length {0} vs {1}", va.len(), vb.len()));
                }
            }
            _ => {
                let sa = serde_json::to_string(a).unwrap_or_default();
                let sb = serde_json::to_string(b).unwrap_or_default();
                let sa_trunc = if sa.len() > 80 { &sa[..80] } else { &sa };
                let sb_trunc = if sb.len() > 80 { &sb[..80] } else { &sb };
                out.push(format!("{path}: {sa_trunc} vs {sb_trunc}"));
            }
        }
    }
}
