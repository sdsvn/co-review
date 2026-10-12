//! Graphify `graph.json` types (networkx node-link format) and the dependency-edge filter.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// A Graphify `graph.json` (networkx node-link format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphifyGraph {
    pub nodes: Vec<GraphifyNodeData>,
    /// Edges in the `links` key (some versions use `edges` instead).
    #[serde(default)]
    pub links: Option<Vec<GraphifyEdge>>,
    /// Edges in the `edges` key (older format).
    #[serde(default)]
    pub edges: Option<Vec<GraphifyEdge>>,
}

impl GraphifyGraph {
    /// All dependency edges, from whichever key is present (`links` or `edges`).
    pub fn all_edges(&self) -> &[GraphifyEdge] {
        self.links
            .as_deref()
            .or(self.edges.as_deref())
            .unwrap_or(&[])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphifyNodeData {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub source_file: Option<String>,
    #[serde(default)]
    pub source_location: Option<String>,
    #[serde(default)]
    pub community: Option<i64>,
    #[serde(default)]
    pub community_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphifyEdge {
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub relation: Option<String>,
}

/// Edge relations that say how code depends on code; `contains` / `method` only restate structure.
pub fn dependency_set() -> HashSet<&'static str> {
    HashSet::from([
        "calls",
        "references",
        "imports",
        "imports_from",
        "implements",
        "indirect_call",
        "inherits",
        "uses",
    ])
}

/// Lazily-initialized set of dependency edge relations matching the TypeScript `DEPENDENCIES`.
///
/// Thread-safe: uses `std::sync::LazyLock`.
pub static DEPENDENCIES: std::sync::LazyLock<HashSet<&'static str>> =
    std::sync::LazyLock::new(dependency_set);
