//! The `co-review` command: start or reuse Co-Review, bridge agents to it over stdio MCP, report its state and logs,
//! and set up agent harnesses.

pub mod agent_setup;
pub mod app;
pub mod debug_log;
pub mod event_stream;
pub mod installation;
pub mod logs;
pub mod mcp_bridge;
pub mod process;
pub mod setup;
pub mod status;
