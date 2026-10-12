//! The MCP server agents connect to as co-reviewers (Claude Code, Codex, Cursor, …): Streamable HTTP at
//! `http://127.0.0.1:<port>/mcp?root=<repo>`, with the tools of `llms.txt`. Port of the Theia app's
//! `co-reviewer-mcp.ts` and `agent-coordination.ts`.
//!
//! [`start`] runs the server on its own tokio runtime thread, so the GPUI app needn't be tokio-based, and records it in
//! `~/.co-review/server.json` for the `co-review` command. The app provides its windows through [`ReviewWindows`] and
//! tells the server when the reviewer closes or reopens one ([`CoReviewer::window_closed`],
//! [`CoReviewer::window_opened`]); it answers the reviewer's button clicks through [`HumanDecisions::resolve`].

mod coordination;
mod paths;
mod payloads;
mod prompts;
mod reviewer;
mod server;
mod services;
mod tools;

pub use coordination::{AgentPresenceTracker, HumanDecisions, Listening};
pub use prompts::{ANSWER_STYLE, ANSWER_STYLE_SHORT, DESIGN_PROMPT, SERVER_INSTRUCTIONS};
pub use reviewer::{ChannelSink, CoReviewer, Session, ToolOutput};
pub use server::{ServerHandle, ServerOptions, call, start};
pub use services::{CodeSyntax, ReviewWindows, SHOWN_NOTE, Services};
