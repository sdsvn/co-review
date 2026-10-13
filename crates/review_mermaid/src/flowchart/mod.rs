//! Flowchart diagram parsing, layout, and rendering.
//!
//! This module implements Mermaid flowchart/graph diagrams with support for:
//! - Multiple flow directions (TD, TB, LR, RL, BT)
//! - Various node shapes (rectangle, diamond, circle, etc.)
//! - Edge styles (solid, dotted, thick)
//! - Arrow types (arrow, circle, cross, bidirectional)
//! - Subgraphs with nesting
//! - Chained edges (A --> B --> C)
//! - Cycle detection and back-edge rendering

pub(crate) mod layout;
pub(crate) mod parser;
pub(crate) mod render;
pub(crate) mod types;
pub(crate) mod utils;

pub use layout::layout_flowchart;
pub use parser::parse_flowchart;
pub use render::{render_flowchart, FlowchartColors};
