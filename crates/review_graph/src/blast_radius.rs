//! Blast radius: what a change touches in the Graphify graph and what depends on it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::graph_model::{GraphifyGraph, DEPENDENCIES};

/// A definition in the graph, for a change's blast radius.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<String>,
}

/// What a change touches and what depends on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlastRadius {
    /// The definitions the change touches.
    pub changed: Vec<GraphNode>,
    /// What calls, imports, references or implements them (two hops), and what they call.
    pub affected: Vec<AffectedNode>,
    /// Dependencies among those, `from` depends on `to`.
    pub edges: Vec<BlastEdge>,
    /// The files of the affected definitions, repository-relative: the change's neighbourhood.
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffectedNode {
    #[serde(flatten)]
    pub node: GraphNode,
    pub hops: u32,
    pub direction: AffectedDirection,
}

/// Declared in the order the TypeScript sorts them: what uses the change first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AffectedDirection {
    #[serde(rename = "uses it")]
    UsesIt,
    #[serde(rename = "it uses")]
    ItUses,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlastEdge {
    pub from: String,
    pub to: String,
}

const MAX_CHANGED: usize = 40;
const MAX_AFFECTED: usize = 60;

/// Compute a change's blast radius from a Graphify graph.
///
/// For each changed line, finds the innermost definition starting at or above that line, then
/// follows dependency edges: callers/importers/implementers two hops out, and callees one hop.
///
/// Returns `None` if the graph has no nodes.
pub fn blast_radius(
    graph: &GraphifyGraph,
    changed_lines: &BTreeMap<String, Vec<u32>>,
) -> Option<BlastRadius> {
    if graph.nodes.is_empty() {
        return None;
    }

    // Build node lookup.
    let node_vec: Vec<GraphNode> = graph
        .nodes
        .iter()
        .filter_map(|n| {
            let source_file = n.source_file.as_ref().filter(|s| !s.is_empty())?;
            let line = n
                .source_location
                .as_deref()
                .and_then(|loc| {
                    loc.chars()
                        .skip_while(|c| !c.is_ascii_digit())
                        .take_while(|c| c.is_ascii_digit())
                        .collect::<String>()
                        .parse::<u32>()
                        .ok()
                })
                .filter(|&l| l > 0);
            Some(GraphNode {
                id: n.id.clone(),
                label: n.label.clone().unwrap_or_else(|| n.id.clone()),
                file: source_file.clone(),
                line,
                area: n.community_name.clone(),
            })
        })
        .collect();

    let mut by_file_map: HashMap<&str, Vec<&GraphNode>> = HashMap::new();
    for node in &node_vec {
        by_file_map
            .entry(node.file.as_str())
            .or_default()
            .push(node);
    }

    let node_lookup: HashMap<&str, &GraphNode> =
        node_vec.iter().map(|n| (n.id.as_str(), n)).collect();

    // Find changed definitions.
    let mut changed = BTreeSet::new();
    for (file, lines) in changed_lines {
        let mut defs: Vec<&GraphNode> = by_file_map
            .get(file.as_str())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|n| n.line.is_some())
            .collect();
        defs.sort_by_key(|n| n.line);

        let file_root = by_file_map
            .get(file.as_str())
            .and_then(|nodes| nodes.iter().find(|n| n.line.is_none() || n.line == Some(1)));

        for &line in lines {
            // The innermost definition starting at or above the line.
            let def = defs
                .iter()
                .rev()
                .find(|n| n.line.is_some_and(|l| l <= line))
                .or(file_root);
            if let Some(def) = def {
                changed.insert(def.id.as_str());
            }
        }
    }

    // Build adjacency.
    let mut depends_on: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut depended_on_by: HashMap<&str, HashSet<&str>> = HashMap::new();

    for edge in graph.all_edges() {
        if DEPENDENCIES.contains(&edge.relation.as_deref().unwrap_or(""))
            && node_lookup.contains_key(edge.source.as_str())
            && node_lookup.contains_key(edge.target.as_str())
            && edge.source != edge.target
        {
            depends_on
                .entry(edge.source.as_str())
                .or_default()
                .insert(edge.target.as_str());
            depended_on_by
                .entry(edge.target.as_str())
                .or_default()
                .insert(edge.source.as_str());
        }
    }

    // BFS: two hops out (callers), one hop in (callees).
    let mut affected: BTreeMap<&str, (u32, AffectedDirection)> = BTreeMap::new();
    let mut all_edges: Vec<BlastEdge> = Vec::new();

    let mut frontier: Vec<&str> = changed.iter().copied().collect();
    for hops in 1..=2u32 {
        let mut next = Vec::new();
        for id in &frontier {
            if let Some(users) = depended_on_by.get(id) {
                for &user in users {
                    all_edges.push(BlastEdge {
                        from: user.to_string(),
                        to: id.to_string(),
                    });
                    if !changed.contains(user) && !affected.contains_key(user) {
                        affected.insert(user, (hops, AffectedDirection::UsesIt));
                        next.push(user);
                    }
                }
            }
        }
        frontier = next;
    }

    for &id in &changed {
        if let Some(used) = depends_on.get(id) {
            for &used_id in used {
                all_edges.push(BlastEdge {
                    from: id.to_string(),
                    to: used_id.to_string(),
                });
                if !changed.contains(used_id) && !affected.contains_key(used_id) {
                    affected.insert(used_id, (1, AffectedDirection::ItUses));
                }
            }
        }
    }

    // Assemble output.
    let changed_nodes: Vec<GraphNode> = changed
        .iter()
        .filter_map(|id| node_lookup.get(id).map(|n| (*n).clone()))
        .take(MAX_CHANGED)
        .collect();

    let mut affected_list: Vec<(&str, u32, AffectedDirection)> = affected
        .into_iter()
        .map(|(id, (hops, dir))| (id, hops, dir))
        .collect();
    affected_list.sort_by_key(|&(_, hops, direction)| (hops, direction));

    let affected_nodes: Vec<AffectedNode> = affected_list
        .into_iter()
        .take(MAX_AFFECTED)
        .filter_map(|(id, hops, direction)| {
            node_lookup.get(id).map(|n| AffectedNode {
                node: (*n).clone(),
                hops,
                direction,
            })
        })
        .collect();

    let shown: HashSet<&str> = changed_nodes
        .iter()
        .map(|n| n.id.as_str())
        .chain(affected_nodes.iter().map(|a| a.node.id.as_str()))
        .collect();

    let filtered_edges: Vec<BlastEdge> = all_edges
        .into_iter()
        .filter(|e| shown.contains(e.from.as_str()) && shown.contains(e.to.as_str()))
        .collect();

    let mut seen = HashSet::new();
    let files: Vec<String> = affected_nodes
        .iter()
        .filter(|a| !changed_lines.contains_key(&a.node.file) && seen.insert(a.node.file.as_str()))
        .map(|a| a.node.file.clone())
        .collect();

    Some(BlastRadius {
        changed: changed_nodes,
        affected: affected_nodes,
        edges: filtered_edges,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_model::GraphifyNodeData;

    fn make_graph() -> GraphifyGraph {
        GraphifyGraph {
            nodes: vec![
                GraphifyNodeData {
                    id: "a".into(),
                    label: Some("func_a".into()),
                    source_file: Some("src/lib.rs".into()),
                    source_location: Some("L10".into()),
                    community: Some(0),
                    community_name: Some("Core".into()),
                },
                GraphifyNodeData {
                    id: "b".into(),
                    label: Some("func_b".into()),
                    source_file: Some("src/lib.rs".into()),
                    source_location: Some("L20".into()),
                    community: Some(0),
                    community_name: Some("Core".into()),
                },
                GraphifyNodeData {
                    id: "c".into(),
                    label: Some("func_c".into()),
                    source_file: Some("src/util.rs".into()),
                    source_location: Some("L5".into()),
                    community: Some(1),
                    community_name: Some("Util".into()),
                },
            ],
            links: Some(vec![
                crate::graph_model::GraphifyEdge {
                    source: "c".into(),
                    target: "a".into(),
                    relation: Some("calls".into()),
                },
                crate::graph_model::GraphifyEdge {
                    source: "a".into(),
                    target: "b".into(),
                    relation: Some("calls".into()),
                },
            ]),
            edges: None,
        }
    }

    #[test]
    fn blast_radius_finds_callers() {
        let graph = make_graph();
        let mut changed_lines = BTreeMap::new();
        changed_lines.insert("src/lib.rs".to_string(), vec![10]);

        let result = blast_radius(&graph, &changed_lines).expect("should produce blast radius");
        assert!(!result.changed.is_empty());
        assert_eq!(result.changed[0].id, "a");

        // c calls a, so c should be affected
        let affected_ids: Vec<&str> = result.affected.iter().map(|a| a.node.id.as_str()).collect();
        assert!(
            affected_ids.contains(&"c"),
            "caller c should be affected, got: {affected_ids:?}"
        );
    }

    #[test]
    fn blast_radius_finds_callees() {
        let graph = make_graph();
        let mut changed_lines = BTreeMap::new();
        changed_lines.insert("src/lib.rs".to_string(), vec![10]);

        let result = blast_radius(&graph, &changed_lines).expect("should produce blast radius");

        // a calls b, so b should be affected with direction "it uses"
        let b_affected = result
            .affected
            .iter()
            .find(|a| a.node.id == "b")
            .expect("callee b should be affected");
        assert_eq!(b_affected.direction, AffectedDirection::ItUses);
    }

    #[test]
    fn blast_radius_empty_graph() {
        let graph = GraphifyGraph {
            nodes: vec![],
            links: None,
            edges: None,
        };
        let changed_lines = BTreeMap::new();
        assert!(blast_radius(&graph, &changed_lines).is_none());
    }
}
