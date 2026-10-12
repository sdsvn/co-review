//! A review's front page as Markdown: the pull request, the agent's overview, the change, every
//! thread grouped by area, and the slow sections (repository overview, blast radius).

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use review_bundle::patch::PatchMeta;
use review_graph::{BlastRadius, GraphNode};
use review_model::{LocationKind, Review, ReviewThread, ThreadStatus, is_overview_page};

use crate::text::{
    basename, file_uri_to_path, js_trim, js_trim_end, relative_path, truncate_utf16,
};

/// What goes on a review's overview page besides the review itself.
#[derive(Debug, Clone, Default)]
pub struct OverviewParts {
    /// The pull request, from the review directory's `PR.md`.
    pub pull_request: Option<PatchMeta>,
    /// The reviewed patches: name, and per file the lines added and removed.
    pub patches: Vec<PatchSummary>,
    /// A repository review: the repository overview (where to start, the areas and how they connect).
    pub repository: Option<String>,
    /// A change review with a repository: its blast radius.
    pub radius: Option<Radius>,
    /// The slow section (`repository` or `radius`) is still being built: the page is written again when it is.
    pub building: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchSummary {
    pub name: String,
    pub files: Vec<PatchFileSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchFileSummary {
    pub path: String,
    pub added: u32,
    pub removed: u32,
}

/// A change's blast radius, or `NoGraph` when Graphify isn't installed or has no graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Radius {
    Graph(BlastRadius),
    NoGraph,
}

/// JavaScript's `\s` (what `String.trim` and regex `\s` match): Unicode White_Space without
/// U+0085, plus U+FEFF.
const JS_SPACE: &str = r"[\s\x{FEFF}&&[^\x{85}]]";

static TITLE_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"^#{JS_SPACE}+[^\n\r\u{{2028}}\u{{2029}}]*\n+"))
        .expect("title regex is valid")
});
static FENCE_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ {0,3}(`{3,}|~{3,})").expect("fence regex is valid"));
static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^#{{1,5}}{JS_SPACE}")).expect("heading regex is valid"));
static HEADING_MARKS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^#+{JS_SPACE}*")).expect("marks regex is valid"));

fn status_label(status: ThreadStatus) -> &'static str {
    match status {
        ThreadStatus::Proposed => "proposed — accept or dismiss it",
        ThreadStatus::Open => "open",
        ThreadStatus::Resolved => "resolved",
    }
}

/// An enum's name as it is serialized (the TypeScript string).
fn wire_name<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        _ => String::new(),
    }
}

struct Where {
    label: String,
    href: Option<String>,
}

/// Where a thread is, as a link the rendered page opens (code at a line, a file, a folder or the repository).
fn thread_where(thread: &ReviewThread, root: &str) -> Where {
    let location = &thread.location;
    if let Some(anchor) = &location.patch_anchor {
        let line = anchor.line.or(anchor.start_line).filter(|line| *line != 0);
        let path = anchor.path.clone().unwrap_or_else(|| {
            let file = location
                .uri
                .as_deref()
                .map(file_uri_to_path)
                .unwrap_or_else(|| "patch".to_string());
            basename(&file).to_string()
        });
        let label = match line {
            Some(line) => format!("{path}:{line}"),
            None => path,
        };
        return Where { label, href: None };
    }
    if let Some(anchor) = &location.doc_anchor {
        return Where {
            label: format!("the document, {}", anchor.describe()),
            href: None,
        };
    }
    let file = location
        .uri
        .as_deref()
        .filter(|uri| uri.starts_with("file:"))
        .map(|uri| relative_path(root, &file_uri_to_path(uri)));
    let Some(file) = file.filter(|file| !file.is_empty() && !file.starts_with("..")) else {
        return Where {
            label: "the repository".to_string(),
            href: None,
        };
    };
    let label = match &location.range {
        Some(range) => {
            let line = range.start.line + 1;
            let end = range.end.line + 1;
            if end != line {
                format!("{file}:{line}-{end}")
            } else {
                format!("{file}:{line}")
            }
        }
        None => file,
    };
    Where {
        href: Some(label.clone()),
        label,
    }
}

/// `code` as a fenced block, its fence longer than any run of backticks in it (so nothing in it can close the block).
fn fence(code: &str) -> String {
    let lines: Vec<&str> = js_trim_end(code).split('\n').collect();
    let mut shown: Vec<String> = lines.iter().take(12).map(|line| line.to_string()).collect();
    if lines.len() > 12 {
        shown.push(format!("… {} more lines", lines.len() - 12));
    }
    let body = shown.join("\n");
    let mut longest = 2;
    let mut run = 0;
    for character in body.chars() {
        if character == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let ticks = "`".repeat(longest + 1);
    format!("{ticks}\n{body}\n{ticks}")
}

/// A section written elsewhere, under this page's `##`: its title dropped, its headings one level down.
fn nest(markdown: &str) -> Vec<String> {
    // The fence that opened the current code block (CommonMark: closed by the same character, at least as long).
    let mut open: Option<String> = None;
    let body = TITLE_LINE.replace(markdown, "");
    body.split('\n')
        .map(|line| {
            let marker = FENCE_MARKER
                .captures(line)
                .and_then(|captures| captures.get(1))
                .map(|marker| marker.as_str());
            if let Some(marker) = marker {
                match &open {
                    None => {
                        open = Some(marker.to_string());
                        return line.to_string();
                    }
                    Some(opened) => {
                        let rest = js_trim(line).get(marker.len()..).unwrap_or("");
                        if marker.as_bytes().first() == opened.as_bytes().first()
                            && marker.len() >= opened.len()
                            && js_trim(rest).is_empty()
                        {
                            open = None;
                            return line.to_string();
                        }
                    }
                }
            }
            if open.is_none() && HEADING.is_match(line) {
                format!("#{line}")
            } else {
                line.to_string()
            }
        })
        .collect()
}

/// A definition as a link into the code (the window's folder is the code).
fn definition(node: &GraphNode) -> String {
    let line = node
        .line
        .filter(|line| *line != 0)
        .map(|line| format!(":{line}"))
        .unwrap_or_default();
    format!("[`{}`]({}{line})", node.label.replace('`', "'"), node.file)
}

/// The blast radius section: what the change touches, what depends on it, as lists and a chart.
fn radius_section(radius: &Radius) -> Vec<String> {
    let radius = match radius {
        Radius::NoGraph => {
            return vec![
                String::new(),
                "## Blast radius".to_string(),
                String::new(),
                "_With [Graphify](https://graphify.net) installed, this lists what else the change affects: \
                 what calls, imports or implements the code it changes._"
                    .to_string(),
            ];
        }
        Radius::Graph(radius) => radius,
    };
    let mut lines = vec![String::new(), "## Blast radius".to_string(), String::new()];
    if radius.changed.is_empty() {
        lines.push(
            "The repository's code graph has no definitions in the changed lines.".to_string(),
        );
        return lines;
    }
    lines.push(format!(
        "**Changed** ({}): {}",
        radius.changed.len(),
        radius
            .changed
            .iter()
            .map(definition)
            .collect::<Vec<_>>()
            .join(", ")
    ));
    if radius.affected.is_empty() {
        lines.push(String::new());
        lines.push(
            "Nothing else in the repository's code graph calls, imports or implements them."
                .to_string(),
        );
        return lines;
    }
    let mut areas: Vec<(&str, Vec<String>)> = Vec::new();
    for affected in &radius.affected {
        let area = affected.node.area.as_deref().unwrap_or("Other");
        let direction = wire_name(&affected.direction);
        let steps = if affected.hops > 1 {
            format!(", {} steps", affected.hops)
        } else {
            String::new()
        };
        let item = format!("{} ({direction}{steps})", definition(&affected.node));
        match areas.iter_mut().find(|(name, _)| *name == area) {
            Some((_, items)) => items.push(item),
            None => areas.push((area, vec![item])),
        }
    }
    lines.push(String::new());
    lines.push(format!(
        "**Affected** ({}), by area: what uses the changed code (up to two steps away), and what it uses.",
        radius.affected.len()
    ));
    lines.push(String::new());
    for (area, items) in &areas {
        lines.push(format!("- **{area}**: {}", items.join(", ")));
    }

    let shown: Vec<(&GraphNode, bool)> = radius
        .changed
        .iter()
        .map(|node| (node, true))
        .chain(
            radius
                .affected
                .iter()
                .map(|affected| (&affected.node, false)),
        )
        .take(30)
        .collect();
    let mut ids: HashMap<&str, String> = HashMap::new();
    for (index, (node, _)) in shown.iter().enumerate() {
        ids.insert(node.id.as_str(), format!("n{index}"));
    }
    let edges: Vec<(&String, &String)> = radius
        .edges
        .iter()
        .filter_map(|edge| Some((ids.get(edge.from.as_str())?, ids.get(edge.to.as_str())?)))
        .take(40)
        .collect();
    if !edges.is_empty() {
        lines.extend(["", "```mermaid", "%% id: blast-radius", "flowchart LR"].map(String::from));
        for (node, changed) in &shown {
            let id = ids
                .get(node.id.as_str())
                .map(String::as_str)
                .unwrap_or_default();
            lines.push(format!(
                "    {id}[\"{}\"]{}",
                node.label.replace('"', "'"),
                if *changed { ":::changed" } else { "" }
            ));
        }
        for (from, to) in edges {
            lines.push(format!("    {from} --> {to}"));
        }
        lines.push("    classDef changed stroke-width:3px".to_string());
        lines.push("```".to_string());
    }
    lines
}

fn pull_request_line(pull_request: &PatchMeta) -> String {
    let mut parts = Vec::new();
    match pull_request.url.as_deref().filter(|url| !url.is_empty()) {
        Some(url) => {
            let text = pull_request.title.clone().unwrap_or_else(|| {
                format!(
                    "#{}",
                    pull_request
                        .number
                        .map(|n| n.to_string())
                        .unwrap_or_default()
                )
            });
            parts.push(format!("[{text}]({url})"));
        }
        None => {
            if let Some(title) = pull_request.title.as_ref().filter(|t| !t.is_empty()) {
                parts.push(title.clone());
            }
        }
    }
    if let (Some(branch), Some(base)) = (&pull_request.branch, &pull_request.base)
        && !branch.is_empty()
        && !base.is_empty()
    {
        parts.push(format!("`{branch}` into `{base}`"));
    }
    if let Some(commits) = pull_request.commits.filter(|commits| *commits != 0) {
        parts.push(format!(
            "{commits} commit{}",
            if commits == 1 { "" } else { "s" }
        ));
    }
    parts.join(" · ")
}

/// A review's front page, the first thing the reviewer reads, for every kind of review: the pull request, the
/// agent's overview, the change itself, then every finding and comment grouped by area, and for a repository review
/// the repository's areas.
pub fn overview_page(review: &Review, root: &str, parts: &OverviewParts) -> String {
    let threads: Vec<&ReviewThread> = review
        .threads
        .iter()
        .filter(|thread| {
            !(thread.location.kind == LocationKind::Document
                && thread.location.uri.as_deref().is_some_and(is_overview_page))
        })
        .collect();
    // A thread's area: its first label, else the file it is on in a patch, else its top-level folder.
    let area_of = |thread: &ReviewThread| -> String {
        if let Some(label) = thread.labels.as_ref().and_then(|labels| labels.first()) {
            return label.clone();
        }
        if let Some(path) = thread
            .location
            .patch_anchor
            .as_ref()
            .and_then(|a| a.path.as_ref())
        {
            return path.clone();
        }
        if thread.location.doc_anchor.is_some() {
            return "The document".to_string();
        }
        let label = thread_where(thread, root).label;
        match label.split_once('/') {
            Some((folder, _)) => folder.to_string(),
            None => "Repository".to_string(),
        }
    };
    let mut sorted = threads.clone();
    sorted.sort_by_key(|thread| thread.number);
    let mut areas: Vec<(String, Vec<&ReviewThread>)> = Vec::new();
    for thread in sorted {
        let area = area_of(thread);
        match areas.iter_mut().find(|(name, _)| *name == area) {
            Some((_, list)) => list.push(thread),
            None => areas.push((area, vec![thread])),
        }
    }
    let count = |status: ThreadStatus| threads.iter().filter(|t| t.status == status).count();
    let agent = review.agent.as_ref().map(|agent| agent.name.as_str());

    let mut lines = vec![format!("# {}", review.title), String::new()];
    let verdict = review
        .verdict
        .as_ref()
        .map(|verdict| {
            format!(
                " Round {} submitted: {}.",
                verdict.count,
                wire_name(&verdict.decision)
            )
        })
        .unwrap_or_default();
    if threads.is_empty() {
        lines.push(format!("No findings yet.{verdict}"));
    } else {
        lines.push(format!(
            "{} thread{}{}: {} proposed, {} open, {} resolved.{verdict} \
             Accept or dismiss findings in the Review panel, open one with its link, or comment on this page.",
            threads.len(),
            if threads.len() == 1 { "" } else { "s" },
            agent
                .map(|agent| format!(", first pass by {agent}"))
                .unwrap_or_default(),
            count(ThreadStatus::Proposed),
            count(ThreadStatus::Open),
            count(ThreadStatus::Resolved),
        ));
    }

    if let Some(pull_request) = &parts.pull_request {
        lines.extend(["", "## The pull request", ""].map(String::from));
        lines.push(pull_request_line(pull_request));
        let body = js_trim(&pull_request.body);
        if !body.is_empty() {
            lines.push(String::new());
            lines.extend(nest(body));
        }
    }

    lines.push(String::new());
    lines.push(
        if review.bundle.is_some() && !parts.patches.is_empty() {
            "## What the change does"
        } else {
            "## Overview"
        }
        .to_string(),
    );
    lines.push(String::new());
    match review.overview.as_deref().map(js_trim).filter(|o| !o.is_empty()) {
        Some(overview) => lines.extend(nest(overview)),
        None => lines.push(format!(
            "_{} has not written an overview{}: what this does, how it works (a flow chart) and what else it affects._",
            agent.unwrap_or("The agent"),
            if agent.is_some() { "" } else { " yet" }
        )),
    }

    for patch in &parts.patches {
        let added: u32 = patch.files.iter().map(|file| file.added).sum();
        let removed: u32 = patch.files.iter().map(|file| file.removed).sum();
        lines.push(String::new());
        lines.push(if parts.patches.len() > 1 {
            format!("## The change: {}", patch.name)
        } else {
            "## The change".to_string()
        });
        lines.push(String::new());
        lines.push(format!(
            "{} file{}, +{added} −{removed}.",
            patch.files.len(),
            if patch.files.len() == 1 { "" } else { "s" }
        ));
        lines.push(String::new());
        for file in &patch.files {
            lines.push(format!(
                "- `{}` +{} −{}",
                file.path, file.added, file.removed
            ));
        }
    }

    if let Some(radius) = &parts.radius {
        lines.extend(radius_section(radius));
    } else if parts.building
        && review
            .bundle
            .as_ref()
            .and_then(|bundle| bundle.repo.as_deref())
            .is_some_and(|repo| !repo.is_empty())
    {
        lines.extend(["", "## Blast radius", ""].map(String::from));
        lines.push(
            "_Co-Review is building the code graph of the repository (Graphify). \
             What else the change affects appears here when it is ready._"
                .to_string(),
        );
    }

    lines.push(String::new());
    lines.push(format!("## Findings and comments ({})", threads.len()));
    if threads.is_empty() {
        lines.push(String::new());
        lines.push("None yet.".to_string());
    }
    for (area, list) in &areas {
        lines.push(String::new());
        lines.push(format!("### {area} ({})", list.len()));
        for thread in list {
            let at = thread_where(thread, root);
            let title = thread
                .messages
                .first()
                .map(|message| {
                    let first_line = message.body.split('\n').next().unwrap_or_default();
                    truncate_utf16(&HEADING_MARKS.replace(first_line, ""), 90)
                })
                .unwrap_or_default();
            lines.push(String::new());
            lines.push(format!("#### #{} {title}", thread.number));
            lines.push(String::new());
            let mut meta = Vec::new();
            if let Some(severity) = &thread.severity {
                meta.push(format!("**{}**", wire_name(severity)));
            }
            meta.push(status_label(thread.status).to_string());
            meta.push(match &at.href {
                Some(href) => format!("[{}]({href})", at.label),
                None => at.label.clone(),
            });
            if let Some(symbol) = thread.location.symbol.as_deref().filter(|s| !s.is_empty()) {
                meta.push(format!("`{symbol}`"));
            }
            lines.push(meta.join(" · "));
            if let Some(anchor) = &thread.location.anchor
                && !js_trim(&anchor.text).is_empty()
            {
                lines.push(String::new());
                lines.push(fence(&anchor.text));
            }
            // Quoted, so a message's own headings don't break the page's sections.
            for message in &thread.messages {
                let body = js_trim(&message.body);
                if body.is_empty() {
                    continue;
                }
                lines.push(String::new());
                let quoted = format!("**{}:** {body}", message.author.name);
                lines.extend(quoted.split('\n').map(|line| format!("> {line}")));
            }
        }
    }

    match parts.repository.as_deref().filter(|r| !r.is_empty()) {
        Some(repository) => {
            lines.extend(["", "## The repository", ""].map(String::from));
            lines.extend(nest(repository));
        }
        None if parts.building && review.bundle.is_none() => {
            lines.extend(["", "## The repository", ""].map(String::from));
            lines.push(
                "_Co-Review is indexing the repository. Where to start, the areas of the code and how \
                 they connect appear here when it is ready._"
                    .to_string(),
            );
        }
        None => {}
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn fence_is_longer_than_any_backtick_run() {
        assert_eq!(fence("has ``` ticks\n"), "````\nhas ``` ticks\n````");
        assert_eq!(fence("plain"), "```\nplain\n```");
    }

    #[test]
    fn fence_shows_twelve_lines() {
        let code = (0..20)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(fence(&code).ends_with("11\n… 8 more lines\n```"));
    }

    #[test]
    fn nest_drops_the_title_and_demotes_headings_outside_code() {
        let nested = nest("# Title\n\n## Section\n```\n## code\n```\n### Sub");
        assert_eq!(nested, ["### Section", "```", "## code", "```", "#### Sub"]);
    }
}
