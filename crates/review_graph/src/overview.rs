//! Graph-based repository overview: most connected code, communities, Mermaid diagram.
//!
//! The repo-map-based fallback (`fromMap` in the TypeScript) lives in `review_index`. Callers
//! pick which path to use; this module only builds the graph overview.

use std::collections::{BTreeMap, HashMap, HashSet};

use review_model::ReviewCoverage;

use crate::graph_model::{GraphifyGraph, DEPENDENCIES};

/// Build the graph-based repository overview Markdown.
///
/// Takes the parsed graph, coverage stats, and the repository's basename for the title.
/// Returns the Markdown lines joined with newlines.
///
/// This is the `fromGraph` path from the TypeScript `repositoryOverview`; the `fromMap` fallback
/// is in `review_index` and takes the outline map as input instead.
pub fn graph_overview(graph: &GraphifyGraph, coverage: &ReviewCoverage, repo_name: &str) -> String {
    let title = format!("# Repository overview: {repo_name}");
    let nodes: HashMap<&str, &crate::graph_model::GraphifyNodeData> =
        graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    // Degree: how many dependency edges touch each node.
    let mut degree: HashMap<&str, u32> = HashMap::new();
    // Between: dependency edges crossing community boundaries.
    // Track insertion order to match TS Map iteration for deterministic sorting.
    let mut between: HashMap<String, u32> = HashMap::new();
    let mut between_order: Vec<String> = Vec::new();
    let deps = &*DEPENDENCIES;

    for edge in graph.all_edges() {
        let rel = edge.relation.as_deref().unwrap_or("");
        if !deps.contains(rel) {
            continue;
        }
        *degree.entry(edge.source.as_str()).or_default() += 1;
        *degree.entry(edge.target.as_str()).or_default() += 1;

        let a = nodes.get(edge.source.as_str()).and_then(|n| n.community);
        let b = nodes.get(edge.target.as_str()).and_then(|n| n.community);
        if let (Some(a), Some(b)) = (a, b) && a != b {
            let key = if a < b {
                format!("{a}-{b}")
            } else {
                format!("{b}-{a}")
            };
            if !between.contains_key(&key) {
                between_order.push(key.clone());
            }
            *between.entry(key).or_default() += 1;
        }
    }

    // Communities. Track insertion order to match TS Map iteration.
    let mut communities: BTreeMap<i64, CommunityInfo> = BTreeMap::new();
    let mut community_order: Vec<i64> = Vec::new();
    for node in &graph.nodes {
        if let Some(community_id) = node.community {
            if !communities.contains_key(&community_id) {
                community_order.push(community_id);
            }
            let entry = communities.entry(community_id).or_insert_with(|| CommunityInfo {
                name: node
                    .community_name
                    .clone()
                    .unwrap_or_else(|| format!("Area {community_id}")),
                nodes: Vec::new(),
            });
            entry.nodes.push(node);
        }
    }
    let community_insertion: HashMap<i64, usize> = community_order
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, i))
        .collect();

    // Filter to communities with >= 3 nodes, sorted by size descending, top 12.
    // Tie-break by insertion order (matching TS Array.sort stability on Map entries).
    let mut areas: Vec<(i64, &CommunityInfo)> = communities
        .iter()
        .filter(|(_, c)| c.nodes.len() >= 3)
        .map(|(&id, c)| (id, c))
        .collect();
    areas.sort_by(|a, b| {
        b.1.nodes.len().cmp(&a.1.nodes.len()).then_with(|| {
            let ia = community_insertion.get(&a.0).copied().unwrap_or(usize::MAX);
            let ib = community_insertion.get(&b.0).copied().unwrap_or(usize::MAX);
            ia.cmp(&ib)
        })
    });
    areas.truncate(12);

    let mut lines = vec![
        title,
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

    if degree.is_empty() {
        lines.push(
            "- No calls, imports or references between definitions yet: start with the areas below."
                .to_string(),
        );
    }

    // Node insertion order (matching TS Map iteration for deterministic tie-breaking).
    let node_order: HashMap<&str, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();

    // Top 8 most connected nodes. Ties broken by the order the node first gained a degree entry
    // in the TS code, which iterates edges in order and inserts into a Map (insertion-ordered).
    // Approximation: use graph.nodes order, since the TS degree map's insertion order follows
    // edge iteration which roughly matches node appearance. We match exactly by tracking edge
    // iteration order.
    let mut degree_order: Vec<&str> = Vec::new();
    {
        let mut seen = HashSet::new();
        for edge in graph.all_edges() {
            let rel = edge.relation.as_deref().unwrap_or("");
            if !deps.contains(rel) {
                continue;
            }
            if seen.insert(edge.source.as_str()) {
                degree_order.push(edge.source.as_str());
            }
            if seen.insert(edge.target.as_str()) {
                degree_order.push(edge.target.as_str());
            }
        }
    }
    let degree_insertion_order: HashMap<&str, usize> = degree_order
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, i))
        .collect();

    let mut degree_sorted: Vec<(&str, u32)> = degree.iter().map(|(&k, &v)| (k, v)).collect();
    degree_sorted.sort_by(|a, b| {
        b.1.cmp(&a.1).then_with(|| {
            let ia = degree_insertion_order.get(a.0).copied().unwrap_or(usize::MAX);
            let ib = degree_insertion_order.get(b.0).copied().unwrap_or(usize::MAX);
            ia.cmp(&ib)
        })
    });
    for &(id, count) in degree_sorted.iter().take(8) {
        if let Some(node) = nodes.get(id) {
            let area = node
                .community
                .and_then(|c| communities.get(&c))
                .map(|c| c.name.as_str());
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

    for &(_, area) in &areas {
        let files: HashSet<&str> = area
            .nodes
            .iter()
            .filter_map(|n| n.source_file.as_deref().filter(|s| !s.is_empty()))
            .collect();
        let file_count = files.len();

        // Top 3 most-connected pieces in this area.
        let mut area_nodes: Vec<&&crate::graph_model::GraphifyNodeData> = area.nodes.iter().collect();
        area_nodes.sort_by(|a, b| {
            let da = degree.get(a.id.as_str()).copied().unwrap_or(0);
            let db = degree.get(b.id.as_str()).copied().unwrap_or(0);
            db.cmp(&da).then_with(|| {
                let ia = node_order.get(a.id.as_str()).copied().unwrap_or(usize::MAX);
                let ib = node_order.get(b.id.as_str()).copied().unwrap_or(usize::MAX);
                ia.cmp(&ib)
            })
        });
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
    let shown_ids: HashSet<i64> = areas.iter().map(|&(id, _)| id).collect();
    let mut cross_edges: Vec<CrossEdge> = between
        .iter()
        .filter_map(|(key, &count)| {
            let mut parts = key.split('-');
            let a: i64 = parts.next()?.parse().ok()?;
            let b: i64 = parts.next()?.parse().ok()?;
            if shown_ids.contains(&a) && shown_ids.contains(&b) && count >= 2 {
                Some(CrossEdge { a, b, count })
            } else {
                None
            }
        })
        .collect();
    let between_insertion: HashMap<String, usize> = between_order
        .iter()
        .enumerate()
        .map(|(i, k)| (k.clone(), i))
        .collect();
    cross_edges.sort_by(|x, y| {
        y.count.cmp(&x.count).then_with(|| {
            let kx = if x.a < x.b {
                format!("{}-{}", x.a, x.b)
            } else {
                format!("{}-{}", x.b, x.a)
            };
            let ky = if y.a < y.b {
                format!("{}-{}", y.a, y.b)
            } else {
                format!("{}-{}", y.b, y.a)
            };
            let ix = between_insertion.get(&kx).copied().unwrap_or(usize::MAX);
            let iy = between_insertion.get(&ky).copied().unwrap_or(usize::MAX);
            ix.cmp(&iy)
        })
    });
    cross_edges.truncate(16);

    if !cross_edges.is_empty() {
        lines.push(String::new());
        lines.push("## How the areas connect".to_string());
        lines.push(String::new());
        lines.push(
            "Lines are calls, references and imports between areas; thicker means more.".to_string(),
        );
        lines.push(String::new());
        lines.push("```mermaid".to_string());
        lines.push("%% id: areas".to_string());
        lines.push("flowchart LR".to_string());

        for &(id, area) in &areas {
            let name = area.name.replace('"', "'");
            lines.push(format!(
                "    a{id}[\"{name} ({})\"]",
                area.nodes.len()
            ));
        }

        for e in &cross_edges {
            let arrow = if e.count >= 10 { "===" } else { "---" };
            lines.push(format!("    a{} {}|{}| a{}", e.a, arrow, e.count, e.b));
        }

        lines.push("```".to_string());
    }

    lines.join("\n")
}

struct CommunityInfo<'a> {
    name: String,
    nodes: Vec<&'a crate::graph_model::GraphifyNodeData>,
}

struct CrossEdge {
    a: i64,
    b: i64,
    count: u32,
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
