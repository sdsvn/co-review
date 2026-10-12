//! ACP agents in review threads: Co-Review launches an agent (Claude Code, Gemini CLI, OpenCode, Goose, Oh My Pi, Pi,
//! Codex or any ACP command) in the repository root and speaks the Agent Client Protocol with it over stdio. Port of
//! the Theia app's `acp-agent-service.ts` and the agent list of `agent-setup.ts`.
//!
//! The UI calls [`AgentService`]: [`AgentService::connect`] a review to an agent,
//! [`AgentService::list_options`] / [`AgentService::set_option`] for its model and reasoning effort,
//! [`AgentService::ask`] when the reviewer asks in a thread, [`AgentService::stop`] for **Stop**. Answers stream into
//! the thread through the review store; [`AgentService::subscribe`] reports each thread update and the agent's state.
//! Permission requests appear on the thread message (`permission`) and are answered with
//! [`review_mcp::HumanDecisions::resolve`].

mod connection;
mod events;
mod presets;
mod prompt;
mod service;
mod settings;
mod turn;

pub use events::{AgentEvent, ConnectionState};
pub use presets::{
    AGENT_PRESETS, AgentPreset, command_line, custom_agent, detect_agents,
    detect_agents_in, find_on_path, is_acp, same_process,
};
pub use prompt::{MAP_CHARS, MAP_WAIT, build_prompt, repository_map, review_root};
pub use service::{AgentService, agent_participant, answers};
pub use settings::{
    AgentSetting, AgentSettingOption, offered_settings, settings_label,
};
