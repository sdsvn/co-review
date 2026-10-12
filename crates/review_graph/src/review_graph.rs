//! Graphify graph runner, blast radius and graph-based repository overview.
//!
//! Graphify (<https://graphify.net>) builds a knowledge graph of a repository: definitions, calls,
//! imports, clusters of code that work together (communities) and the most connected code. It uses
//! Tree-sitter only (no LLM, no API key), and `graphify update <repo>` takes seconds.
//!
//! This crate owns:
//! - Running `graphify update`, one build per repository root at a time.
//! - Freshness: skip rebuilds when HEAD + working-tree status are unchanged.
//! - Worktree seeding: copy-on-write clone of `graphify-out/` from a sibling worktree.
//! - Blast radius: what a change touches and what depends on it.
//! - Graph-based overview: most connected code, communities, Mermaid diagram.
//!
//! The repo-map-based fallback overview lives in `review_index`.

mod blast_radius;
mod build;
mod graph_model;
mod overview;

pub use blast_radius::{blast_radius, BlastRadius, GraphNode};
pub use build::{build_graph, graphify_installed, BuildState, GraphState, FRESH_MS};
pub use graph_model::{GraphifyEdge, GraphifyGraph, GraphifyNodeData, DEPENDENCIES};
pub use overview::graph_overview;
