/// Parsing of `<patch>.comments.json` / `.comments.jsonl` pre-seeded findings.
use serde::{Deserialize, Serialize};

/// A suggested edit proposed alongside a finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub before: String,
    pub after: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(rename = "startLine", skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
}

/// A pre-seeded finding from a `.comments.json` or `.comments.jsonl` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// Findings without an id are skipped when a findings file is synced.
    #[serde(default)]
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal: Option<Proposal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default)]
    pub anchor: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default)]
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
}

/// Parse findings from a JSON array string (`.comments.json`).
pub fn read_findings_json(text: &str) -> Result<Vec<Finding>, serde_json::Error> {
    serde_json::from_str(text)
}

/// Parse findings from a JSONL string (`.comments.jsonl`), one JSON object per line.
pub fn read_findings_jsonl(text: &str) -> Result<Vec<Finding>, serde_json::Error> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(serde_json::from_str)
        .collect()
}
