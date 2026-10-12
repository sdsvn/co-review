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
    let mut findings = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        findings.push(serde_json::from_str(trimmed)?);
    }
    Ok(findings)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_read_findings_json() {
        let json = r#"[{"id":"f1","body":"issue here","anchor":{}}]"#;
        let findings = read_findings_json(json).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "f1");
        assert_eq!(findings[0].body, "issue here");
    }

    #[test]
    fn test_read_findings_jsonl() {
        let jsonl = r#"{"id":"f1","body":"issue 1","anchor":{}}
{"id":"f2","body":"issue 2","anchor":{},"severity":"high"}
"#;
        let findings = read_findings_jsonl(jsonl).unwrap();
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[1].severity, Some("high".to_string()));
    }

    #[test]
    fn test_read_findings_jsonl_empty_lines() {
        let jsonl = "\n  \n{\"id\":\"f1\",\"body\":\"x\",\"anchor\":{}}\n\n";
        let findings = read_findings_jsonl(jsonl).unwrap();
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn test_finding_with_proposal() {
        let json = r#"[{"id":"f1","body":"fix this","anchor":{},"proposal":{"before":"old","after":"new","path":"src/main.rs","startLine":10}}]"#;
        let findings = read_findings_json(json).unwrap();
        assert_eq!(findings[0].proposal.as_ref().unwrap().before, "old");
        assert_eq!(findings[0].proposal.as_ref().unwrap().start_line, Some(10));
    }
}
