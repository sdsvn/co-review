//! Graph-based repository overview: most connected code, communities, Mermaid diagram.
//!
//! The TypeScript sorts entries of insertion-ordered `Map`s with a stable sort, so ties keep the
//! order in which a key first appeared; the `Vec`s with an index below keep that order.

use std::collections::{HashMap, HashSet};

use review_model::ReviewCoverage;

use crate::graph_model::{DEPENDENCIES, GraphifyGraph, GraphifyNodeData};

/// Counts by key, in the order each key first appeared.
struct OrderedCounts<K> {
    index: HashMap<K, usize>,
    entries: Vec<(K, u32)>,
}

impl<K: std::hash::Hash + Eq + Clone> OrderedCounts<K> {
    fn new() -> Self {
        Self {
            index: HashMap::new(),
            entries: Vec::new(),
        }
    }

    fn add(&mut self, key: K) {
        let at = *self.index.entry(key.clone()).or_insert_with(|| {
            self.entries.push((key, 0));
            self.entries.len() - 1
        });
        self.entries[at].1 += 1;
    }

    fn get(&self, key: &K) -> u32 {
        self.index.get(key).map_or(0, |&at| self.entries[at].1)
    }
}

/// The repository overview (Markdown) from a Graphify graph: the TypeScript `fromGraph`.
pub fn graph_overview(graph: &GraphifyGraph, coverage: &ReviewCoverage, repo_name: &str) -> String {
    let nodes: HashMap<&str, &GraphifyNodeData> =
        graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    // Degree: how many dependency edges touch each node. Between: dependency edges crossing
    // community boundaries.
    let mut degree = OrderedCounts::new();
    let mut between = OrderedCounts::new();
    for edge in graph.all_edges() {
        if !DEPENDENCIES.contains(&edge.relation.as_deref().unwrap_or("")) {
            continue;
        }
        degree.add(edge.source.as_str());
        degree.add(edge.target.as_str());
        let a = nodes.get(edge.source.as_str()).and_then(|n| n.community);
        let b = nodes.get(edge.target.as_str()).and_then(|n| n.community);
        if let (Some(a), Some(b)) = (a, b)
            && a != b
        {
            between.add(if a < b {
                format!("{a}-{b}")
            } else {
                format!("{b}-{a}")
            });
        }
    }

    let mut community_index: HashMap<i64, usize> = HashMap::new();
    let mut communities: Vec<(i64, CommunityInfo)> = Vec::new();
    for node in &graph.nodes {
        if let Some(community_id) = node.community {
            let at = *community_index.entry(community_id).or_insert_with(|| {
                communities.push((
                    community_id,
                    CommunityInfo {
                        name: node
                            .community_name
                            .clone()
                            .unwrap_or_else(|| format!("Area {community_id}")),
                        nodes: Vec::new(),
                    },
                ));
                communities.len() - 1
            });
            communities[at].1.nodes.push(node);
        }
    }

    let mut areas: Vec<&(i64, CommunityInfo)> =
        communities.iter().filter(|(_, c)| c.nodes.len() >= 3).collect();
    areas.sort_by_key(|(_, c)| std::cmp::Reverse(c.nodes.len()));
    areas.truncate(12);

    let mut lines = vec![
        format!("# Repository overview: {repo_name}"),
        String::new(),
        format!(
            "Built from the Graphify graph of this repository (`graphify-out/graph.json`: {} definitions, \
             {} areas). {} of {} source files viewed so far. \
             Graphify's own report, [GRAPH_REPORT.md](graphify-out/GRAPH_REPORT.md), adds the import cycles and the surprising \
             connections between distant parts of the code.",
            graph.nodes.len(),
            communities.len(),
            coverage.viewed,
            coverage.total
        ),
        String::new(),
        "## Start here".to_string(),
        String::new(),
        "The most connected code: what the rest of the repository calls, imports or implements."
            .to_string(),
        String::new(),
    ];

    if degree.entries.is_empty() {
        lines.push(
            "- No calls, imports or references between definitions yet: start with the areas below."
                .to_string(),
        );
    }

    let mut most_connected = degree.entries.clone();
    most_connected.sort_by_key(|&(_, count)| std::cmp::Reverse(count));
    for &(id, count) in most_connected.iter().take(8) {
        if let Some(node) = nodes.get(id) {
            let area = node
                .community
                .and_then(|c| community_index.get(&c))
                .map(|&at| communities[at].1.name.as_str());
            let label = node.label.as_deref().unwrap_or(id);
            let link_text = link(label, node.source_file.as_deref(), node.source_location.as_deref());
            let area_suffix = area.map_or(String::new(), |a| format!(", in {a}"));
            lines.push(format!("- {link_text}: {count} connections{area_suffix}"));
        }
    }

    lines.push(String::new());
    lines.push("## Areas".to_string());
    lines.push(String::new());
    lines.push(
        "Clusters of code that work together (Graphify communities), largest first, with their most used pieces."
            .to_string(),
    );
    lines.push(String::new());

    for (_, area) in &areas {
        let file_count = area
            .nodes
            .iter()
            .filter_map(|n| n.source_file.as_deref().filter(|s| !s.is_empty()))
            .collect::<HashSet<_>>()
            .len();
        // The 3 most connected pieces in this area.
        let mut area_nodes = area.nodes.clone();
        area_nodes.sort_by_key(|n| std::cmp::Reverse(degree.get(&n.id.as_str())));
        let key: Vec<String> = area_nodes
            .iter()
            .take(3)
            .map(|n| {
                link(
                    n.label.as_deref().unwrap_or(&n.id),
                    n.source_file.as_deref(),
                    n.source_location.as_deref(),
                )
            })
            .collect();
        let plural = if file_count == 1 { "" } else { "s" };
        lines.push(format!(
            "- **{}** ({file_count} file{plural}): {}",
            area.name,
            key.join(", ")
        ));
    }

    // Mermaid diagram of inter-area connections.
    let shown_ids: HashSet<i64> = areas.iter().map(|(id, _)| *id).collect();
    let mut cross_edges: Vec<(i64, i64, u32)> = between
        .entries
        .iter()
        .filter_map(|(key, count)| {
            let mut parts = key.split('-');
            let a: i64 = parts.next()?.parse().ok()?;
            let b: i64 = parts.next()?.parse().ok()?;
            (shown_ids.contains(&a) && shown_ids.contains(&b) && *count >= 2).then_some((a, b, *count))
        })
        .collect();
    cross_edges.sort_by_key(|&(_, _, count)| std::cmp::Reverse(count));
    cross_edges.truncate(16);

    if !cross_edges.is_empty() {
        lines.extend(
            [
                "",
                "## How the areas connect",
                "",
                "Lines are calls, references and imports between areas; thicker means more.",
                "",
                "```mermaid",
                "%% id: areas",
                "flowchart LR",
            ]
            .map(String::from),
        );
        for (id, area) in &areas {
            let name = area.name.replace('"', "'");
            lines.push(format!("    a{id}[\"{name} ({})\"]", area.nodes.len()));
        }
        for (a, b, count) in &cross_edges {
            let arrow = if *count >= 10 { "===" } else { "---" };
            lines.push(format!("    a{a} {arrow}|{count}| a{b}"));
        }
        lines.push("```".to_string());
    }

    lines.join("\n")
}

struct CommunityInfo<'a> {
    name: String,
    nodes: Vec<&'a GraphifyNodeData>,
}

/// A Markdown link that opens code at a location (`path:line`).
fn link(label: &str, file: Option<&str>, location: Option<&str>) -> String {
    let Some(file) = file.filter(|f| !f.is_empty()) else {
        return format!("`{label}`");
    };
    let cleaned_label = label.replace(['[', ']'], "");
    let line = location.and_then(|loc| {
        loc.chars()
            .skip_while(|c| *c != 'L')
            .skip(1)
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<u32>()
            .ok()
    });
    let suffix = match line {
        Some(l) if l > 1 => format!(":{l}"),
        _ => String::new(),
    };
    // URI-encode the file path (matching TS encodeURI behaviour for typical paths)
    let encoded = file.replace(' ', "%20");
    format!("[{cleaned_label}]({encoded}{suffix})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_model::{GraphifyEdge, GraphifyNodeData};

    fn sample_graph() -> GraphifyGraph {
        GraphifyGraph {
            nodes: vec![
                GraphifyNodeData {
                    id: "a".into(),
                    label: Some("main".into()),
                    source_file: Some("src/main.rs".into()),
                    source_location: Some("L1".into()),
                    community: Some(0),
                    community_name: Some("Entry".into()),
                },
                GraphifyNodeData {
                    id: "b".into(),
                    label: Some("serve".into()),
                    source_file: Some("src/server.rs".into()),
                    source_location: Some("L10".into()),
                    community: Some(0),
                    community_name: Some("Entry".into()),
                },
                GraphifyNodeData {
                    id: "c".into(),
                    label: Some("handle".into()),
                    source_file: Some("src/server.rs".into()),
                    source_location: Some("L20".into()),
                    community: Some(0),
                    community_name: Some("Entry".into()),
                },
            ],
            links: Some(vec![GraphifyEdge {
                source: "a".into(),
                target: "b".into(),
                relation: Some("calls".into()),
            }]),
            edges: None,
        }
    }

    #[test]
    fn overview_contains_title_and_areas() {
        let graph = sample_graph();
        let coverage = ReviewCoverage {
            total: 10,
            viewed: 2,
            areas: vec![],
        };
        let md = graph_overview(&graph, &coverage, "my-repo");
        assert!(md.contains("# Repository overview: my-repo"));
        assert!(md.contains("## Areas"));
        assert!(md.contains("Entry"));
    }

    #[test]
    fn overview_shows_connections() {
        let graph = sample_graph();
        let coverage = ReviewCoverage {
            total: 10,
            viewed: 0,
            areas: vec![],
        };
        let md = graph_overview(&graph, &coverage, "test");
        // a and b have connections
        assert!(md.contains("connections"));
    }

    #[test]
    fn link_formats_correctly() {
        assert_eq!(link("foo", Some("a.rs"), Some("L5")), "[foo](a.rs:5)");
        assert_eq!(link("foo", Some("a.rs"), Some("L1")), "[foo](a.rs)");
        assert_eq!(link("foo", None, None), "`foo`");
        assert_eq!(link("[bar]", Some("b.rs"), None), "[bar](b.rs)");
    }
}
