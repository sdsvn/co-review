//! Design-document format: parsing, step tree, and format warnings.
//!
//! Ports `extensions/review/src/common/design-format.ts` with identical output.
//! A design document declares `co-review: design` in its frontmatter and is checked
//! against the contract described in `docs/design-docs.md`.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

/// The frontmatter value that declares a design document.
const DESIGN_FORMAT: &str = "design";

/// The headings of the contract's sections, in canonical order.
const DESIGN_SECTIONS: [&str; 6] = [
    "Context",
    "Design",
    "Alternatives",
    "Behaviour",
    "Diagrams",
    "Open Questions",
];

/// Index of the contract section a lower-cased heading names (`behavior` is an alias).
fn design_section_index(key: &str) -> Option<usize> {
    let key = if key == "behavior" { "behaviour" } else { key };
    DESIGN_SECTIONS
        .iter()
        .position(|heading| heading.to_lowercase() == key)
}

/// A parsed `## ` section.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub heading: String,
    /// Lower-cased heading.
    pub key: String,
    pub body: String,
}

/// Result of splitting a document body into sections.
#[derive(Debug, Clone, PartialEq)]
pub struct SectionsResult {
    pub preamble: String,
    pub sections: Vec<Section>,
}

/// Frontmatter key-value pairs and the remaining body.
#[derive(Debug, Clone, PartialEq)]
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
            let val = COMMENT_RE.replace(&kv_caps[2], "");
            let unquoted = ['\'', '"'].iter().find_map(|quote| {
                val.strip_prefix(*quote)
                    .and_then(|rest| rest.strip_suffix(*quote))
            });
            data.insert(
                kv_caps[1].to_lowercase(),
                unquoted.unwrap_or(&val).to_string(),
            );
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

/// `## ` sections in document order; `preamble` is what precedes the first one
/// (the `# ` title excluded).
pub fn sections_of(body: &str) -> SectionsResult {
    let mut sections: Vec<Section> = Vec::new();
    let mut preamble: Vec<&str> = Vec::new();
    let mut fence = false;

    for line in body.split('\n') {
        if fence_marker(line).is_some() {
            fence = !fence;
        }
        if let Some(caps) = (!fence).then(|| HEADING_RE.captures(line)).flatten() {
            sections.push(Section {
                heading: caps[1].to_string(),
                key: caps[1].to_lowercase(),
                body: String::new(),
            });
            continue;
        }
        if let Some(section) = sections.last_mut() {
            section.body.push_str(line);
            section.body.push('\n');
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
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub text: String,
    pub indent: i32,
    pub depth: u32,
    pub children: Vec<Step>,
}

// JS `.` excludes both `\n` and `\r`; Rust regex `.` excludes only `\n`.
// Use `[^\r\n]` to match JS behavior with CRLF input.
static BULLET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([ \t]*)- ([^\r\n]*)$").expect("valid regex"));

/// Indented `- ` bullets to tree, nesting by relative indent (tabs = 4); continuation lines fold up.
pub fn steps(body: &str) -> Vec<Step> {
    /// Closes the open steps indented at least `indent` into their parents (the root's is -1).
    fn close(stack: &mut Vec<Step>, indent: i32) {
        while let Some(step) = stack.pop_if(|step| step.indent >= indent) {
            if let Some(parent) = stack.last_mut() {
                parent.children.push(step);
            }
        }
    }

    let mut stack = vec![Step {
        text: String::new(),
        indent: -1,
        depth: 0,
        children: Vec::new(),
    }];
    for line in body.split('\n') {
        if let Some(caps) = BULLET_RE.captures(line) {
            let indent = caps[1].replace('\t', "    ").len() as i32;
            close(&mut stack, indent);
            let depth = stack.last().map_or(0, |parent| parent.depth) + 1;
            stack.push(Step {
                text: caps[2].trim().to_string(),
                indent,
                depth,
                children: Vec::new(),
            });
        } else if !line.trim().is_empty()
            && let Some(last) = stack.last_mut().filter(|step| step.depth > 0)
        {
            last.text.push(' ');
            last.text.push_str(line.trim());
        }
    }
    close(&mut stack, 0);
    stack.pop().map(|root| root.children).unwrap_or_default()
}

/// The document's rendering mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    Design,
    Plain,
}

/// How a document renders, and where a declared document departs from the contract.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DocumentFormat {
    pub mode: Mode,
    /// The frontmatter declares `co-review: design`.
    pub declared: bool,
    /// Where a declared document departs from the contract.
    pub warnings: Vec<String>,
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
    let declared_value = frontmatter.data.get("co-review");
    let declared =
        declared_value.is_some_and(|value| value.trim().to_lowercase() == DESIGN_FORMAT);
    let sections_result = sections_of(&frontmatter.body);
    let sections = &sections_result.sections;

    let detected =
        PSEUDOCODE_RE.is_match(file_name) || sections.iter().any(|s| s.key == "design");

    let mut warnings: Vec<String> = Vec::new();

    if let Some(val) = declared_value.filter(|_| !declared) {
        warnings.push(format!(
            "Unknown `co-review: {val}` \u{2014} the only format is `co-review: {DESIGN_FORMAT}`; rendered as plain Markdown."
        ));
    }

    if declared {
        if !TITLE_CHECK_RE.is_match(&frontmatter.body) && !frontmatter.data.contains_key("title") {
            warnings.push("No `# ` title (or `title:` in the frontmatter).".to_string());
        }

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

        let extra: Vec<String> = sections
            .iter()
            .filter(|s| design_section_index(&s.key).is_none())
            .map(|s| format!("`## {}`", s.heading))
            .collect();
        if !extra.is_empty() {
            warnings.push(format!(
                "Sections outside the contract (shown as plain Markdown): {}.",
                extra.join(", ")
            ));
        }

        let order: Vec<usize> = sections
            .iter()
            .filter_map(|s| design_section_index(&s.key))
            .collect();
        if order.windows(2).any(|w| w[1] < w[0]) {
            warnings.push(format!(
                "Sections are out of order ({}).",
                DESIGN_SECTIONS.join(", ")
            ));
        }
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

/// The fence marker a line opens or closes a code block with (leading whitespace allowed).
fn fence_marker(line: &str) -> Option<&'static str> {
    ["```", "~~~"]
        .into_iter()
        .find(|marker| line.trim_start().starts_with(marker))
}

/// Strip fenced code blocks from text. The TS regex pairs ``` only with ``` and ~~~ only
/// with ~~~ (backreference `\1`); the closer must be just the fence marker on its own line.
fn strip_fenced_blocks(text: &str) -> String {
    let mut result = String::new();
    let mut fence: Option<&str> = None;
    for line in text.split('\n') {
        match fence {
            None => {
                fence = fence_marker(line);
                if fence.is_none() {
                    result.push_str(line);
                    result.push('\n');
                }
            }
            Some(marker) if line.trim() == marker => fence = None,
            Some(_) => {}
        }
    }
    result
}

static FENCE_LANG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(?:```|~~~)\s*([0-9A-Za-z_]*)").expect("valid regex"));

/// Extract the language tags of opening code fences in a section body.
/// The TS code matches all fence openers, then filters to even indices (0, 2, 4...)
/// to get "opening" fences. We replicate that exact behavior.
fn opening_fence_languages(body: &str) -> Vec<String> {
    FENCE_LANG_RE
        .captures_iter(body)
        .step_by(2)
        .map(|caps| caps[1].to_string())
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

    let prose: String = sections
        .iter()
        .map(|s| strip_fenced_blocks(&s.body))
        .collect::<Vec<_>>()
        .join("\n");
    let prose = URL_RE.replace_all(&prose, "").to_string();

    let examples: Vec<String> = LINE_REF_RE
        .find_iter(&prose)
        .take(3)
        .map(|r| format!("`{}`", r.as_str()))
        .collect();
    if !examples.is_empty() {
        warnings.push(format!(
            "Line references ({}): a person reads this; describe the code in words and name only the top-level module or function.",
            examples.join(", ")
        ));
    }

    if sections
        .iter()
        .flat_map(|s| opening_fence_languages(&s.body))
        .any(|lang| lang.to_lowercase() != "mermaid")
    {
        warnings.push("Code blocks other than Mermaid: describe the behaviour in words; the code belongs in the implementation.".to_string());
    }

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

    // UTF-16 length, as JavaScript's `.length`.
    let long_count = all_steps
        .iter()
        .filter(|s| s.text.encode_utf16().count() > LONG_STEP)
        .count();
    if long_count > 0 {
        let plural = if long_count > 1 { "s are" } else { " is" };
        warnings.push(format!(
            "{long_count} step{plural} longer than two lines: keep one decision per step, and move details into child steps."
        ));
    }

    // Steps each code name appears in, in first-seen order (JS Map iteration order).
    let mut uses: Vec<(String, usize)> = Vec::new();
    for step in &all_steps {
        let mut in_step = HashSet::new();
        for caps in CODE_NAME_RE.captures_iter(&step.text) {
            let name = &caps[1];
            if !in_step.insert(name.to_string()) {
                continue;
            }
            match uses.iter_mut().find(|(used, _)| used == name) {
                Some((_, count)) => *count += 1,
                None => uses.push((name.to_string(), 1)),
            }
        }
    }
    let repeated: Vec<String> = uses
        .iter()
        .filter(|(_, n)| *n >= REPEATED_NAME)
        .take(3)
        .map(|(name, n)| format!("`{name}` ({n} steps)"))
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
mod tests {
    use super::*;

    #[test]
    fn test_truncate_utf16() {
        assert_eq!(truncate_utf16("hello world", 5), "hello");
        assert_eq!(truncate_utf16("\u{1F600}abc", 1), "");
        assert_eq!(truncate_utf16("\u{1F600}abc", 2), "\u{1F600}");
    }
}
