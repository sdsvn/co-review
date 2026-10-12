//! Design-document format: parsing, step tree, and format warnings.
//!
//! Ports `extensions/review/src/common/design-format.ts` with identical output.
//! A design document declares `co-review: design` in its frontmatter and is checked
//! against the contract described in `docs/design-docs.md`.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// The frontmatter value that declares a design document.
pub const DESIGN_FORMAT: &str = "design";

/// A contract section, in canonical order.
#[derive(Debug, Clone)]
pub struct DesignSectionDef {
    pub key: &'static str,
    pub heading: &'static str,
    pub tree: bool,
    pub aliases: &'static [&'static str],
}

/// The canonical sections of the design-document contract.
pub static DESIGN_SECTIONS: &[DesignSectionDef] = &[
    DesignSectionDef {
        key: "context",
        heading: "Context",
        tree: false,
        aliases: &[],
    },
    DesignSectionDef {
        key: "design",
        heading: "Design",
        tree: true,
        aliases: &[],
    },
    DesignSectionDef {
        key: "alternatives",
        heading: "Alternatives",
        tree: false,
        aliases: &[],
    },
    DesignSectionDef {
        key: "behaviour",
        heading: "Behaviour",
        tree: false,
        aliases: &["behavior"],
    },
    DesignSectionDef {
        key: "diagrams",
        heading: "Diagrams",
        tree: false,
        aliases: &[],
    },
    DesignSectionDef {
        key: "open questions",
        heading: "Open Questions",
        tree: true,
        aliases: &[],
    },
];

/// Index of the contract section a heading key names (its key or an alias), or -1.
pub fn design_section_index(key: &str) -> i32 {
    for (i, section) in DESIGN_SECTIONS.iter().enumerate() {
        if section.key == key || section.aliases.contains(&key) {
            return i as i32;
        }
    }
    -1
}

/// A parsed `## ` section.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Section {
    pub heading: String,
    /// Lower-cased heading.
    pub key: String,
    pub body: String,
}

/// Result of splitting a document body into sections.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SectionsResult {
    pub preamble: String,
    pub sections: Vec<Section>,
}

/// Frontmatter key-value pairs and the remaining body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Frontmatter {
    pub data: HashMap<String, String>,
    pub body: String,
}

static FRONTMATTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^---[ \t]*\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)").expect("valid regex"));
static KV_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9A-Za-z_-]+)\s*:\s*(.*?)\s*$").expect("valid regex"));
static COMMENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+#.*$").expect("valid regex"));

/// Splits leading YAML frontmatter (flat `key: value` lines) from the body.
pub fn split_frontmatter(markdown: &str) -> Frontmatter {
    let Some(caps) = FRONTMATTER_RE.captures(markdown) else {
        return Frontmatter {
            data: HashMap::new(),
            body: markdown.to_string(),
        };
    };

    let full_match = caps.get(0).expect("match exists");
    let inner = &caps[1];
    let mut data = HashMap::new();
    for line in inner.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(kv_caps) = KV_RE.captures(line) {
            let key = kv_caps[1].to_lowercase();
            let mut val = kv_caps[2].to_string();
            // Strip trailing inline comment
            val = COMMENT_RE.replace(&val, "").to_string();
            // Strip surrounding matching quotes
            let bytes = val.as_bytes();
            if bytes.len() >= 2
                && (bytes[0] == b'\'' || bytes[0] == b'"')
                && bytes[bytes.len() - 1] == bytes[0]
            {
                val = val[1..val.len() - 1].to_string();
            }
            data.insert(key, val);
        }
    }

    Frontmatter {
        data,
        body: markdown[full_match.end()..].to_string(),
    }
}

// JS `.` excludes `\r`; use `[^\r\n]` so headings don't capture trailing `\r`.
static HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^##\s+([^\r\n]+?)\s*#*\s*$").expect("valid regex"));
static TITLE_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^#\s+").expect("valid regex"));
static FENCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(```|~~~)").expect("valid regex"));

/// `## ` sections in document order; `preamble` is what precedes the first one
/// (the `# ` title excluded).
pub fn sections_of(body: &str) -> SectionsResult {
    let mut sections: Vec<Section> = Vec::new();
    let mut preamble: Vec<&str> = Vec::new();
    let mut current: Option<usize> = None;
    let mut fence = false;

    for line in body.split('\n') {
        if FENCE_RE.is_match(line) {
            fence = !fence;
        }

        if let Some(caps) = (!fence).then(|| HEADING_RE.captures(line)).flatten() {
                let heading = caps[1].to_string();
                let key = heading.to_lowercase();
                sections.push(Section {
                    heading,
                    key,
                    body: String::new(),
                });
                current = Some(sections.len() - 1);
                continue;
        }

        if let Some(idx) = current {
            sections[idx].body.push_str(line);
            sections[idx].body.push('\n');
        } else if !TITLE_LINE_RE.is_match(line) {
            preamble.push(line);
        }
    }

    SectionsResult {
        preamble: preamble.join("\n"),
        sections,
    }
}

/// A step in the design tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Step {
    pub text: String,
    pub indent: i32,
    pub depth: u32,
    pub children: Vec<Step>,
}

/// Compute indentation width treating tabs as 4 spaces.
fn indent_width(whitespace: &str) -> i32 {
    whitespace.replace('\t', "    ").len() as i32
}

// JS `.` excludes both `\n` and `\r`; Rust regex `.` excludes only `\n`.
// Use `[^\r\n]` to match JS behavior with CRLF input.
static BULLET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([ \t]*)- ([^\r\n]*)$").expect("valid regex"));

/// Indented `- ` bullets to tree, nesting by relative indent (tabs = 4); continuation lines fold up.
pub fn steps(body: &str) -> Vec<Step> {
    struct StackEntry {
        indent: i32,
        depth: u32,
        children: Vec<Step>,
        text: String,
    }

    let mut stack: Vec<StackEntry> = vec![StackEntry {
        indent: -1,
        depth: 0,
        children: Vec::new(),
        text: String::new(),
    }];

    for line in body.split('\n') {
        if let Some(caps) = BULLET_RE.captures(line) {
            let indent = indent_width(&caps[1]);
            let text = caps[2].trim().to_string();

            // Pop stack entries with indent >= current
            while stack.len() > 1 && stack.last().is_some_and(|e| e.indent >= indent) {
                let entry = stack.pop().expect("stack not empty");
                let node = Step {
                    text: entry.text,
                    indent: entry.indent,
                    depth: entry.depth,
                    children: entry.children,
                };
                stack
                    .last_mut()
                    .expect("stack not empty")
                    .children
                    .push(node);
            }

            let parent_depth = stack.last().map_or(0, |e| e.depth);
            stack.push(StackEntry {
                indent,
                depth: parent_depth + 1,
                children: Vec::new(),
                text,
            });
        } else if !line.trim().is_empty() && stack.len() > 1 {
            let last = stack.last_mut().expect("stack has >1 entries");
            last.text.push(' ');
            last.text.push_str(line.trim());
        }
    }

    // Flush remaining stack entries
    while stack.len() > 1 {
        let entry = stack.pop().expect("stack not empty");
        let node = Step {
            text: entry.text,
            indent: entry.indent,
            depth: entry.depth,
            children: entry.children,
        };
        stack
            .last_mut()
            .expect("stack not empty")
            .children
            .push(node);
    }

    stack.pop().expect("root exists").children
}

/// Whether a frontmatter value declares the design format.
pub fn is_design_format(value: Option<&str>) -> bool {
    value.unwrap_or("").trim().to_lowercase() == DESIGN_FORMAT
}

/// The document's rendering mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    Design,
    Plain,
}

/// How a document renders, and where a declared document departs from the contract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocumentFormat {
    pub mode: Mode,
    /// The frontmatter declares `co-review: design`.
    pub declared: bool,
    /// Where a declared document departs from the contract.
    pub warnings: Vec<String>,
}

/// Count UTF-16 code units, matching JavaScript's `.length`.
fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Truncate a string to at most `max` UTF-16 code units, returning a valid str slice.
fn truncate_utf16(s: &str, max: usize) -> &str {
    let mut utf16_count = 0;
    for (byte_idx, ch) in s.char_indices() {
        let ch_utf16_len = ch.len_utf16();
        if utf16_count + ch_utf16_len > max {
            return &s[..byte_idx];
        }
        utf16_count += ch_utf16_len;
    }
    s
}

static WHITESPACE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+").expect("valid regex"));

/// Stable step id: marker/format-stripped text, whitespace-collapsed, capped at 80 UTF-16 code units.
pub fn step_id(text: &str) -> String {
    let stripped: String = text.chars().filter(|&c| c != '`' && c != '*').collect();
    let collapsed = WHITESPACE_RE.replace_all(&stripped, " ");
    let trimmed = collapsed.trim();
    truncate_utf16(trimmed, 80).to_string()
}

static PSEUDOCODE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\.pseudocode\.md$").expect("valid regex"));
static TITLE_CHECK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^#\s+\S").expect("valid regex"));
static WRONG_MARKERS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*([*+]|[0-9]+[.)]\s)").expect("valid regex"));

/// How a document renders, and -- when it declares the contract -- where it departs from it.
pub fn document_format(markdown: &str, file_name: &str) -> DocumentFormat {
    let frontmatter = split_frontmatter(markdown);
    let declared_value = frontmatter.data.get("co-review").cloned();
    let declared = is_design_format(declared_value.as_deref());
    let sections_result = sections_of(&frontmatter.body);
    let sections = &sections_result.sections;

    let detected =
        PSEUDOCODE_RE.is_match(file_name) || sections.iter().any(|s| s.key == "design");

    let mut warnings: Vec<String> = Vec::new();

    if let Some(ref val) = declared_value.as_ref().filter(|_| !declared) {
        warnings.push(format!(
            "Unknown `co-review: {val}` \u{2014} the only format is `co-review: {DESIGN_FORMAT}`; rendered as plain Markdown."
        ));
    }

    if declared {
        // Check for title
        if !TITLE_CHECK_RE.is_match(&frontmatter.body) && !frontmatter.data.contains_key("title") {
            warnings.push("No `# ` title (or `title:` in the frontmatter).".to_string());
        }

        // Check for Design section
        let design = sections.iter().find(|s| s.key == "design");
        if let Some(design_section) = design {
            let design_steps = steps(&design_section.body);

            if design_steps.is_empty() {
                if WRONG_MARKERS_RE.is_match(&design_section.body) {
                    warnings.push("`## Design` uses `*`, `+` or numbered items; steps must be `- ` bullets.".to_string());
                } else {
                    warnings.push("`## Design` has no `- ` step list.".to_string());
                }
            } else if WRONG_MARKERS_RE.is_match(&design_section.body) {
                warnings.push("`## Design` mixes `- ` with `*`, `+` or numbered items; only `- ` items become steps.".to_string());
            }
        } else {
            warnings.push("No `## Design` section.".to_string());
        }

        // Extra sections
        let extra: Vec<String> = sections
            .iter()
            .filter(|s| design_section_index(&s.key) < 0)
            .map(|s| format!("`## {}`", s.heading))
            .collect();
        if !extra.is_empty() {
            warnings.push(format!(
                "Sections outside the contract (shown as plain Markdown): {}.",
                extra.join(", ")
            ));
        }

        // Order check
        let order: Vec<i32> = sections
            .iter()
            .map(|s| design_section_index(&s.key))
            .filter(|&i| i >= 0)
            .collect();
        let out_of_order = order.windows(2).any(|w| w[1] < w[0]);
        if out_of_order {
            let section_names: Vec<&str> = DESIGN_SECTIONS.iter().map(|s| s.heading).collect();
            warnings.push(format!(
                "Sections are out of order ({}).",
                section_names.join(", ")
            ));
        }

        // Readability warnings
        warnings.extend(readability_warnings(sections));
    }

    let mode = if declared || (detected && declared_value.is_none()) {
        Mode::Design
    } else {
        Mode::Plain
    };

    DocumentFormat {
        mode,
        declared,
        warnings,
    }
}

/// Steps longer than this (about two lines) usually hold more than one decision.
const LONG_STEP: usize = 220;
/// A code name in this many steps or more is repeated rather than introduced once.
const REPEATED_NAME: usize = 3;

static FENCE_OPEN_BACKTICK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*```").expect("valid regex"));
static FENCE_OPEN_TILDE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*~~~").expect("valid regex"));
static FENCE_CLOSE_BACKTICK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*```\s*$").expect("valid regex"));
static FENCE_CLOSE_TILDE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*~~~\s*$").expect("valid regex"));

/// Strip fenced code blocks from text. The TS regex pairs ``` only with ``` and ~~~ only
/// with ~~~ (backreference `\1`); the closer must be just the fence marker on its own line.
fn strip_fenced_blocks(text: &str) -> String {
    let mut result = String::new();
    let mut fence_type: Option<FenceType> = None;

    for line in text.split('\n') {
        match fence_type {
            None => {
                if FENCE_OPEN_BACKTICK_RE.is_match(line) {
                    fence_type = Some(FenceType::Backtick);
                } else if FENCE_OPEN_TILDE_RE.is_match(line) {
                    fence_type = Some(FenceType::Tilde);
                } else {
                    result.push_str(line);
                    result.push('\n');
                }
            }
            Some(FenceType::Backtick) => {
                if FENCE_CLOSE_BACKTICK_RE.is_match(line) {
                    fence_type = None;
                }
            }
            Some(FenceType::Tilde) => {
                if FENCE_CLOSE_TILDE_RE.is_match(line) {
                    fence_type = None;
                }
            }
        }
    }

    result
}

#[derive(Clone, Copy)]
enum FenceType {
    Backtick,
    Tilde,
}

static FENCE_LANG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(?:```|~~~)\s*([0-9A-Za-z_]*)").expect("valid regex"));

/// Extract the language tags of opening code fences in a section body.
/// The TS code matches all fence openers, then filters to even indices (0, 2, 4...)
/// to get "opening" fences. We replicate that exact behavior.
fn opening_fence_languages(body: &str) -> Vec<String> {
    let all_matches: Vec<String> = FENCE_LANG_RE
        .captures_iter(body)
        .map(|caps| caps[1].to_string())
        .collect();
    // Filter to even indices (the "opening" fences)
    all_matches
        .into_iter()
        .enumerate()
        .filter(|(i, _)| i % 2 == 0)
        .map(|(_, lang)| lang)
        .collect()
}

static URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?-u:\b)https?://\S+").expect("valid regex"));
static LINE_REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)[0-9A-Za-z_./-]+\.[a-z]{1,5}:[0-9]+|#L[0-9]+").expect("valid regex"));
static CODE_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`([^`?\u{26a0}\u{270e}]+)`").expect("valid regex"));

/// Readability warnings for declared design documents.
fn readability_warnings(sections: &[Section]) -> Vec<String> {
    let mut warnings: Vec<String> = Vec::new();

    // Build prose: strip fenced code blocks from each section body, join, strip URLs
    let prose: String = sections
        .iter()
        .map(|s| strip_fenced_blocks(&s.body))
        .collect::<Vec<_>>()
        .join("\n");
    let prose = URL_RE.replace_all(&prose, "").to_string();

    // Line references
    let line_refs: Vec<String> = LINE_REF_RE
        .find_iter(&prose)
        .map(|m| m.as_str().to_string())
        .collect();
    if !line_refs.is_empty() {
        let examples: Vec<String> = line_refs
            .iter()
            .take(3)
            .map(|r| format!("`{r}`"))
            .collect();
        warnings.push(format!(
            "Line references ({}): a person reads this; describe the code in words and name only the top-level module or function.",
            examples.join(", ")
        ));
    }

    // Non-mermaid code blocks
    let code_langs: Vec<String> = sections
        .iter()
        .flat_map(|s| opening_fence_languages(&s.body))
        .collect();
    if code_langs
        .iter()
        .any(|lang| lang.to_lowercase() != "mermaid")
    {
        warnings.push("Code blocks other than Mermaid: describe the behaviour in words; the code belongs in the implementation.".to_string());
    }

    // Collect all steps from the Design section
    let design = sections.iter().find(|s| s.key == "design");
    let design_body = design.map_or("", |s| s.body.as_str());
    let design_steps = steps(design_body);
    let mut all_steps: Vec<&Step> = Vec::new();
    fn collect_steps<'a>(nodes: &'a [Step], out: &mut Vec<&'a Step>) {
        for step in nodes {
            out.push(step);
            collect_steps(&step.children, out);
        }
    }
    collect_steps(&design_steps, &mut all_steps);

    // Long steps (using UTF-16 length to match JS)
    let long_count = all_steps
        .iter()
        .filter(|s| utf16_len(&s.text) > LONG_STEP)
        .count();
    if long_count > 0 {
        let plural = if long_count > 1 { "s are" } else { " is" };
        warnings.push(format!(
            "{long_count} step{plural} longer than two lines: keep one decision per step, and move details into child steps."
        ));
    }

    // Repeated code names: track insertion order to match JS Map iteration order
    let mut uses: HashMap<String, usize> = HashMap::new();
    for step in &all_steps {
        let names: HashSet<String> = CODE_NAME_RE
            .captures_iter(&step.text)
            .map(|caps| caps[1].to_string())
            .collect();
        for name in names {
            *uses.entry(name).or_insert(0) += 1;
        }
    }
    // Rebuild in first-seen order
    let mut ordered_names: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for step in &all_steps {
        let names: Vec<String> = CODE_NAME_RE
            .captures_iter(&step.text)
            .map(|caps| caps[1].to_string())
            .collect();
        let mut step_seen: HashSet<String> = HashSet::new();
        for name in names {
            if step_seen.insert(name.clone()) && seen.insert(name.clone()) {
                ordered_names.push(name);
            }
        }
    }

    let repeated: Vec<String> = ordered_names
        .iter()
        .filter(|name| uses.get(*name).copied().unwrap_or(0) >= REPEATED_NAME)
        .take(3)
        .map(|name| {
            let n = uses.get(name).copied().unwrap_or(0);
            format!("`{name}` ({n} steps)")
        })
        .collect();
    if !repeated.is_empty() {
        warnings.push(format!(
            "Code names repeated across steps: {}. Name each once where it is introduced, then refer to it in words.",
            repeated.join(", ")
        ));
    }

    warnings
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_split_frontmatter_basic() {
        let md = "---\nco-review: design\ntitle: Test\n---\n# Hello\n";
        let fm = split_frontmatter(md);
        assert_eq!(fm.data.get("co-review").unwrap(), "design");
        assert_eq!(fm.data.get("title").unwrap(), "Test");
        assert_eq!(fm.body, "# Hello\n");
    }

    #[test]
    fn test_split_frontmatter_none() {
        let md = "# Just a heading\n";
        let fm = split_frontmatter(md);
        assert!(fm.data.is_empty());
        assert_eq!(fm.body, md);
    }

    #[test]
    fn test_sections_of() {
        let body = "# Title\nSome text\n## Context\nContext body\n## Design\n- Step 1\n";
        let result = sections_of(body);
        assert_eq!(result.sections.len(), 2);
        assert_eq!(result.sections[0].key, "context");
        assert_eq!(result.sections[1].key, "design");
    }

    #[test]
    fn test_steps_basic() {
        let body = "- Step 1\n  - Sub step\n    - Deep step\n- Step 2\n";
        let tree = steps(body);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].text, "Step 1");
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].text, "Sub step");
        assert_eq!(tree[0].children[0].children.len(), 1);
    }

    #[test]
    fn test_steps_continuation() {
        let body = "- Start of step\n  continued here\n";
        let tree = steps(body);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].text, "Start of step continued here");
    }

    #[test]
    fn test_step_id() {
        assert_eq!(
            step_id("Use `RateLimiter` for **rate limiting**"),
            "Use RateLimiter for rate limiting"
        );
    }

    #[test]
    fn test_step_id_truncation() {
        let long = "a".repeat(100);
        assert_eq!(step_id(&long).len(), 80);
    }

    #[test]
    fn test_design_section_index() {
        assert_eq!(design_section_index("design"), 1);
        assert_eq!(design_section_index("behavior"), 3);
        assert_eq!(design_section_index("unknown"), -1);
    }

    #[test]
    fn test_is_design_format() {
        assert!(is_design_format(Some("design")));
        assert!(is_design_format(Some(" Design ")));
        assert!(!is_design_format(Some("other")));
        assert!(!is_design_format(None));
    }

    #[test]
    fn test_document_format_declared() {
        let md = "---\nco-review: design\n---\n# Title\n## Design\n- Step 1\n";
        let fmt = document_format(md, "test.md");
        assert_eq!(fmt.mode, Mode::Design);
        assert!(fmt.declared);
        assert!(fmt.warnings.is_empty());
    }

    #[test]
    fn test_document_format_detected_pseudocode() {
        let md = "# Title\n## Design\n- Step 1\n";
        let fmt = document_format(md, "test.pseudocode.md");
        assert_eq!(fmt.mode, Mode::Design);
        assert!(!fmt.declared);
        assert!(fmt.warnings.is_empty());
    }

    #[test]
    fn test_utf16_len_ascii() {
        assert_eq!(utf16_len("hello"), 5);
    }

    #[test]
    fn test_utf16_len_emoji() {
        // Emoji U+1F600 is 2 UTF-16 code units
        assert_eq!(utf16_len("\u{1F600}"), 2);
    }

    #[test]
    fn test_truncate_utf16() {
        assert_eq!(truncate_utf16("hello world", 5), "hello");
        assert_eq!(truncate_utf16("\u{1F600}abc", 1), "");
        assert_eq!(truncate_utf16("\u{1F600}abc", 2), "\u{1F600}");
    }
}
