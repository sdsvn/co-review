/// Unified-diff parsing and patch metadata (paths and line numbers as in the patch).
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// A single row in a parsed patch hunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchRow {
    /// `hunk`, `ctx`, `add`, or `del`.
    pub t: String,
    /// Old-side line number (present for `ctx` and `del`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub o: Option<u32>,
    /// New-side line number (present for `ctx` and `add`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    /// The text content of this row (without the leading +/-/space).
    pub s: String,
}

/// A file entry in a parsed patch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchFile {
    pub path: String,
    pub tag: String,
    pub added: u32,
    pub removed: u32,
    pub rows: Vec<PatchRow>,
}

/// Metadata parsed from a `PR.md` or `<patch>.md` header.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub body: String,
}

/// The ordered list of document names to look for in a review directory.
pub const DOCUMENT_NAMES: &[&str] = &[
    "index.markdown",
    "index.md",
    "INDEX.md",
    "README.md",
    "readme.md",
];

fn strip_diff_prefix(p: &str) -> String {
    let cleaned = p.trim();
    let cleaned = if cleaned.starts_with('"') && cleaned.ends_with('"') && cleaned.len() >= 2 {
        &cleaned[1..cleaned.len() - 1]
    } else {
        cleaned
    };
    for pre in &["a/", "b/", "i/", "w/", "c/", "o/"] {
        if let Some(rest) = cleaned.strip_prefix(pre) {
            return rest.to_string();
        }
    }
    cleaned.to_string()
}

fn hunk_starts(line: &str) -> (u32, u32) {
    let mut body = line.to_string();
    if let Some(idx) = body[2..].find("@@") {
        body.truncate(idx + 4);
    }
    let mut old_start: u32 = 1;
    let mut new_start: u32 = 1;
    let cleaned = body.replace('@', "");
    for tok in cleaned.split_whitespace() {
        if tok.len() < 2 {
            continue;
        }
        let sign = tok.as_bytes()[0];
        let rest = &tok[1..];
        let num_str = rest.split(',').next().unwrap_or(rest);
        let val = match num_str.parse::<u32>() {
            Ok(v) => v,
            Err(_) => continue,
        };
        if sign == b'-' {
            old_start = val;
        }
        if sign == b'+' {
            new_start = val;
        }
    }
    (old_start, new_start)
}

fn new_file(path: String) -> PatchFile {
    PatchFile {
        path,
        tag: "modified".to_string(),
        added: 0,
        removed: 0,
        rows: Vec::new(),
    }
}

/// The tag a file's header line gives it.
fn file_tag(line: &str) -> Option<&'static str> {
    if line.starts_with("new file mode") {
        Some("new file")
    } else if line.starts_with("deleted file mode") {
        Some("deleted")
    } else if line.starts_with("rename from ") || line.starts_with("rename to ") {
        Some("renamed")
    } else if line.starts_with("Binary files") {
        Some("binary")
    } else {
        None
    }
}

/// Parse a unified diff (git diff, format-patch, or plain unified diff) into file entries.
pub fn parse_patch(src: &str) -> Vec<PatchFile> {
    let mut files: Vec<PatchFile> = Vec::new();
    let mut current: Option<PatchFile> = None;
    let mut old_no: u32 = 0;
    let mut new_no: u32 = 0;

    // Match TS: split on \r?\n — do NOT use .lines() since we need the trailing empty element.
    let normalized = src.replace("\r\n", "\n");
    for line in normalized.split('\n') {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(file) = current.take() {
                files.push(file);
            }
            let parts: Vec<&str> = rest.split_whitespace().collect();
            let path = if parts.len() == 2 {
                strip_diff_prefix(parts[1])
            } else {
                strip_diff_prefix(&parts.join(" "))
            };
            current = Some(new_file(path));
        } else if current.is_none() && (line.starts_with("--- ") || line.starts_with("+++ ")) {
            if let Some(rest) = line.strip_prefix("+++ ") {
                current = Some(new_file(strip_diff_prefix(rest.trim())));
            }
        } else if let Some(tag) = file_tag(line) {
            if let Some(cur) = current.as_mut() {
                cur.tag = tag.to_string();
            }
        } else if line.starts_with("--- ") || line.starts_with("+++ ") {
            if let (Some(cur), Some(rest)) = (&mut current, line.strip_prefix("+++ ")) {
                let p = strip_diff_prefix(rest.trim());
                if !p.is_empty() && p != "/dev/null" {
                    cur.path = p;
                }
            }
        } else if line.starts_with("@@") {
            if let Some(cur) = current.as_mut() {
                (old_no, new_no) = hunk_starts(line);
                cur.rows.push(PatchRow {
                    t: "hunk".to_string(),
                    o: None,
                    n: None,
                    s: line.to_string(),
                });
            }
        } else if let Some(cur) = current.as_mut().filter(|cur| !cur.rows.is_empty()) {
            let (t, s) = if line.is_empty() {
                ("ctx", "")
            } else if let Some(rest) = line.strip_prefix('+') {
                ("add", rest)
            } else if let Some(rest) = line.strip_prefix('-') {
                ("del", rest)
            } else if let Some(rest) = line.strip_prefix(' ') {
                ("ctx", rest)
            } else {
                continue;
            };
            cur.rows.push(PatchRow {
                t: t.to_string(),
                o: (t != "add").then_some(old_no),
                n: (t != "del").then_some(new_no),
                s: s.to_string(),
            });
            match t {
                "add" => {
                    new_no += 1;
                    cur.added += 1;
                }
                "del" => {
                    old_no += 1;
                    cur.removed += 1;
                }
                _ => {
                    old_no += 1;
                    new_no += 1;
                }
            }
        }
    }
    if let Some(file) = current.take() {
        files.push(file);
    }
    files
}

/// Parse `key: value` lines (optionally inside a `---` fence), a blank line, then Markdown.
pub fn parse_patch_meta(text: &str) -> PatchMeta {
    // The TS does text.replace(/^---\s*\n/, '') which strips a leading "---" fence line.
    let text = if let Some(first_newline) = text.find('\n') {
        let first_line = &text[..first_newline];
        if first_line.starts_with("---") && first_line[3..].chars().all(|c| c.is_whitespace()) {
            &text[first_newline + 1..]
        } else {
            text
        }
    } else {
        text
    };

    let lines: Vec<&str> = text.split('\n').collect();
    let mut meta = PatchMeta::default();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "---" {
            i += 1;
            break;
        }
        // Match /^(\w+):\s*(.*)$/
        if let Some(colon_pos) = line.find(':') {
            let key = &line[..colon_pos];
            if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                let value = line[colon_pos + 1..].trim_start();
                match key {
                    "title" => meta.title = Some(value.to_string()),
                    "pr" | "number" => {
                        meta.number = value.parse::<u32>().ok().filter(|&v| v != 0);
                    }
                    "branch" => meta.branch = Some(value.to_string()),
                    "base" => meta.base = Some(value.to_string()),
                    "commits" => {
                        meta.commits = value.parse::<u32>().ok().filter(|&v| v != 0);
                    }
                    "repo" => meta.repo = Some(value.trim().to_string()),
                    "url" => meta.url = Some(value.trim().to_string()),
                    _ => {}
                }
                i += 1;
                continue;
            }
        }
        break;
    }

    // Explicit ASCII char class to match JS \w behavior.
    static PULL_URL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"github\.com/([a-zA-Z0-9_.\-]+/[a-zA-Z0-9_.\-]+)/pull/(\d+)")
            .expect("static regex")
    });
    if let Some(caps) = meta.url.as_deref().and_then(|url| PULL_URL.captures(url)) {
        meta.repo = meta.repo.or_else(|| Some(caps[1].to_string()));
        meta.number = meta.number.or_else(|| caps[2].parse().ok());
    }

    meta.body = lines[i..].join("\n").trim().to_string();
    meta
}

/// The lines of the new side that a patch changes, per file: added lines, and for removed
/// lines the new-side line where they were (the next line kept or added).
pub fn changed_lines(files: &[PatchFile]) -> BTreeMap<String, Vec<u32>> {
    let mut changed = BTreeMap::new();
    for file in files {
        let mut lines = BTreeSet::new();
        let mut removed = false;
        for row in &file.rows {
            // JS falsy: `row.n` counts only when non-zero.
            let n = row.n.filter(|&n| n != 0);
            match row.t.as_str() {
                "add" => lines.extend(n),
                "ctx" if removed => lines.extend(n),
                _ => {}
            }
            removed = row.t == "del";
        }
        if !lines.is_empty() {
            changed.insert(file.path.clone(), lines.into_iter().collect());
        }
    }
    changed
}

