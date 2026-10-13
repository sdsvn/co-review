//! The repository overview: where to start, the areas of the code and how they connect. Built from
//! the Graphify graph when the repository has one (`review_graph::graph_overview`), otherwise from
//! the Tree-sitter repo map.

use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context as _, Result};
use regex::Regex;
use review_graph::{GraphState, GraphifyGraph, encode_uri, graph_overview};
use review_model::ReviewCoverage;

use crate::text::{basename, is_js_space};

/// Files that look like where a program starts.
pub static ENTRY_POINT: LazyLock<Regex> = LazyLock::new(|| {
    // ASCII-only case folding, as JavaScript's `/…/i` without the `u` flag.
    Regex::new(r"(?i-u)(^|/)(main|index|server|app|cli|cmd)\.[a-z]+$")
        .expect("entry point regex is valid")
});

/// What the repository overview is built from.
pub struct OverviewInput<'a> {
    pub root: &'a Path,
    pub coverage: &'a ReviewCoverage,
    /// Repository-relative file and its outline line (`Symbol { member, … }, function, …`), sorted by path.
    pub outlines: &'a [(String, String)],
    /// Whether Co-Review could build the Graphify graph.
    pub graph: Option<GraphState>,
}

/// The repository overview page (Markdown): from `graphify-out/graph.json` when it has definitions,
/// otherwise from the repo map. Fails, as the TypeScript does, when `graph.json` is not JSON.
pub fn repository_overview(input: &OverviewInput) -> Result<String> {
    let graph_file = input.root.join("graphify-out").join("graph.json");
    let graph = match std::fs::read_to_string(&graph_file) {
        Ok(text) => {
            let value: serde_json::Value = serde_json::from_str(&text)
                .with_context(|| format!("reading {}", graph_file.display()))?;
            serde_json::from_value::<GraphifyGraph>(value)
                .ok()
                .filter(|graph| !graph.nodes.is_empty())
        }
        Err(_) => None,
    };
    Ok(match graph {
        Some(graph) => graph_overview(&graph, input.coverage, &repository_name(input.root)),
        None => map_overview(input),
    })
}

fn repository_name(root: &Path) -> String {
    basename(&root.to_string_lossy()).to_string()
}

/// A Markdown link that opens the file (resolved from the repository root).
fn link(label: &str, file: &str) -> String {
    format!("[{}]({})", label.replace(['[', ']'], ""), encode_uri(file))
}

/// An outline's top-level definitions: split at commas that are not inside a `{ … }` member list.
fn definitions(outline: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while let Some(offset) = outline[index..].find(',') {
        let comma = index + offset;
        let rest = &outline[comma + 1..];
        let inside_braces = rest
            .find(['{', '}'])
            .is_some_and(|brace| rest[brace..].starts_with('}'));
        if inside_braces {
            index = comma + 1;
            continue;
        }
        parts.push(&outline[start..comma]);
        let after_space = rest.len() - rest.trim_start_matches(is_js_space).len();
        start = comma + 1 + after_space;
        index = start;
    }
    parts.push(&outline[start..]);
    parts
}

/// The overview from the Tree-sitter repo map: files, folders and their definitions.
pub fn map_overview(input: &OverviewInput) -> String {
    let files: Vec<&str> = input
        .outlines
        .iter()
        .map(|(file, _)| file.as_str())
        .collect();
    let outline_of = |file: &str| {
        input
            .outlines
            .iter()
            .find(|(name, _)| name == file)
            .map_or("", |(_, outline)| outline.as_str())
    };
    let entry: Vec<&str> = files
        .iter()
        .copied()
        .filter(|file| ENTRY_POINT.is_match(file))
        .take(6)
        .collect();
    let mut by_size: Vec<&(String, String)> = input.outlines.iter().collect();
    by_size.sort_by_key(|(_, outline)| std::cmp::Reverse(outline.split(',').count()));
    let rich: Vec<&str> = by_size
        .iter()
        .take(4)
        .map(|(file, _)| file.as_str())
        .collect();

    let coverage = input.coverage;
    let mut lines = vec![
        format!("# Repository overview: {}", repository_name(input.root)),
        String::new(),
        format!(
            "Built from the Tree-sitter repo map: {} source files. {} of {} viewed so far.",
            files.len(),
            coverage.viewed,
            coverage.total
        ),
        if input.graph == Some(GraphState::Failed) {
            "Building the Graphify graph failed (see the Co-Review log); `graphify update .` in the repository shows why."
                .to_string()
        } else {
            "For calls, imports and clusters of related code, install [Graphify](https://graphify.net) \
             (`uv tool install graphifyy` or `pipx install graphifyy`) and reopen this page: Co-Review builds \
             the graph itself (code only, no LLM, seconds)."
                .to_string()
        },
        String::new(),
        "## Start here".to_string(),
        String::new(),
    ];
    if let Some(readme) = ["README.md", "readme.md", "README"]
        .into_iter()
        .find(|readme| input.root.join(readme).exists())
    {
        lines.push(format!(
            "- {}: what the project says about itself",
            link(readme, readme)
        ));
    }
    for file in &entry {
        lines.push(format!("- {}: looks like an entry point", link(file, file)));
    }
    for file in rich.iter().filter(|file| !entry.contains(file)) {
        let count = definitions(outline_of(file))
            .into_iter()
            .filter(|definition| !definition.is_empty())
            .count();
        lines.push(format!(
            "- {}: one of the largest files ({count} top-level definitions)",
            link(file, file)
        ));
    }
    lines.extend(["", "## Areas", "", "Folders, with what they define.", ""].map(String::from));
    for area in &coverage.areas {
        let prefix = format!("{}/", area.path);
        let names: Vec<&str> = files
            .iter()
            .filter(|file| {
                if area.path == "." {
                    !file.contains('/')
                } else {
                    file.starts_with(&prefix)
                }
            })
            .flat_map(|file| {
                definitions(outline_of(file))
                    .into_iter()
                    .map(|definition| definition.split(" {").next().unwrap_or_default())
            })
            .filter(|name| !name.is_empty())
            .take(6)
            .collect();
        let defines = if names.is_empty() {
            String::new()
        } else {
            format!(". Defines {}", names.join(", "))
        };
        let next = area
            .next
            .as_deref()
            .filter(|next| !next.is_empty())
            .map(|next| format!(". Next: {}", link(basename(next), next)))
            .unwrap_or_default();
        lines.push(format!(
            "- **{}**: {} file{}, {} viewed{defines}{next}.",
            if area.path == "." {
                "(root)"
            } else {
                &area.path
            },
            area.total,
            if area.total == 1 { "" } else { "s" },
            area.viewed
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_split_outside_member_lists() {
        assert_eq!(definitions("A { b, c }, d,  e"), ["A { b, c }", "d", "e"]);
        assert_eq!(definitions(""), [""]);
    }

    #[test]
    fn entry_points() {
        assert!(ENTRY_POINT.is_match("src/Main.RS"));
        assert!(ENTRY_POINT.is_match("index.ts"));
        assert!(!ENTRY_POINT.is_match("src/domain.ts"));
    }
}
