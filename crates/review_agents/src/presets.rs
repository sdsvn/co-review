//! The ACP agents Co-Review knows, and which of them are installed.

use std::path::{Path, PathBuf};

use review_model::{AgentConfig, AgentTransport};

/// An agent known to speak ACP over stdio. It is offered only when its command, and the agent CLI it wraps
/// (`requires`), are on the PATH.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub command: &'static str,
    pub args: &'static [&'static str],
    pub requires: Option<&'static str>,
}

pub const AGENT_PRESETS: &[AgentPreset] = &[
    AgentPreset {
        id: "claude",
        name: "Claude Code",
        command: "npx",
        args: &["-y", "@agentclientprotocol/claude-agent-acp"],
        requires: None,
    },
    AgentPreset {
        id: "gemini",
        name: "Gemini CLI",
        command: "gemini",
        args: &["--experimental-acp"],
        requires: None,
    },
    AgentPreset {
        id: "opencode",
        name: "OpenCode",
        command: "opencode",
        args: &["acp"],
        requires: None,
    },
    AgentPreset {
        id: "goose",
        name: "Goose",
        command: "goose",
        args: &["acp"],
        requires: None,
    },
    AgentPreset {
        id: "omp",
        name: "Oh My Pi",
        command: "omp",
        args: &["acp"],
        requires: None,
    },
    AgentPreset {
        id: "pi",
        name: "Pi",
        command: "npx",
        args: &["-y", "pi-acp"],
        requires: Some("pi"),
    },
    AgentPreset {
        id: "codex",
        name: "Codex",
        command: "npx",
        args: &["-y", "@zed-industries/codex-acp"],
        requires: Some("codex"),
    },
];

impl AgentPreset {
    pub fn config(&self) -> AgentConfig {
        AgentConfig {
            id: self.id.to_string(),
            name: self.name.to_string(),
            transport: Some(AgentTransport::Acp),
            command: Some(self.command.to_string()),
            args: Some(self.args.iter().map(|arg| arg.to_string()).collect()),
            settings: None,
            settings_label: None,
            extra: serde_json::Map::new(),
        }
    }
}

/// The presets whose commands are installed, in preset order, as agent configs.
pub fn detect_agents() -> Vec<AgentConfig> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    detect_agents_in(&path)
}

/// [`detect_agents`] with `path` as the PATH.
pub fn detect_agents_in(path: &std::ffi::OsStr) -> Vec<AgentConfig> {
    let installed = |command: &str| find_on_path(command, path).is_some();
    AGENT_PRESETS
        .iter()
        .filter(|preset| {
            installed(preset.command) && preset.requires.is_none_or(installed)
        })
        .map(AgentPreset::config)
        .collect()
}

/// Where `command` would be found on `path`, like `which`.
pub fn find_on_path(command: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    if command.contains(std::path::MAIN_SEPARATOR) || command.contains('/') {
        let file = PathBuf::from(command);
        return is_executable(&file).then_some(file);
    }
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_string())
            .split(';')
            .map(str::to_string)
            .chain(std::iter::once(String::new()))
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(path).find_map(|directory| {
        extensions.iter().find_map(|extension| {
            let file = directory.join(format!("{command}{extension}"));
            is_executable(&file).then_some(file)
        })
    })
}

fn is_executable(file: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(file) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

/// An agent config for any ACP command line the reviewer typed (`my-agent --acp "two words"`). Simple shell quoting
/// (single and double quotes, backslash escapes) is understood. `None` for an empty line.
pub fn custom_agent(command_line: &str) -> Option<AgentConfig> {
    let mut words = split_command_line(command_line).into_iter();
    let command = words.next()?;
    let name = Path::new(&command)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| command.clone());
    Some(AgentConfig {
        id: "custom".to_string(),
        name,
        transport: Some(AgentTransport::Acp),
        command: Some(command),
        args: Some(words.collect()),
        settings: None,
        settings_label: None,
        extra: serde_json::Map::new(),
    })
}

fn split_command_line(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut characters = line.chars();
    while let Some(character) = characters.next() {
        match (quote, character) {
            (Some(open), _) if character == open => quote = None,
            (Some('"') | None, '\\') => {
                if let Some(escaped) = characters.next() {
                    word.push(escaped);
                }
                in_word = true;
            }
            (Some(_), _) => word.push(character),
            (None, '"' | '\'') => {
                quote = Some(character);
                in_word = true;
            }
            (None, _) if character.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, _) => {
                word.push(character);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// Whether the review's agent is launched by Co-Review over ACP (as opposed to an MCP agent in its own harness).
pub fn is_acp(agent: Option<&AgentConfig>) -> bool {
    agent.is_some_and(|agent| {
        agent.transport.unwrap_or(AgentTransport::Acp) == AgentTransport::Acp
            && agent
                .command
                .as_deref()
                .is_some_and(|command| !command.is_empty())
    })
}

/// `command args…`, as shown to the reviewer.
pub fn command_line(agent: &AgentConfig) -> String {
    std::iter::once(agent.command.clone().unwrap_or_default())
        .chain(agent.args.clone().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

/// Whether two configs run the same agent process (they may differ in settings only).
pub fn same_process(first: &AgentConfig, second: &AgentConfig) -> bool {
    first.id == second.id
        && first.transport == second.transport
        && command_line(first) == command_line(second)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn splits_command_lines() {
        assert_eq!(
            split_command_line(
                r#"my-agent --acp "two words" 'single \ quoted' esc\ aped"#
            ),
            vec![
                "my-agent",
                "--acp",
                "two words",
                "single \\ quoted",
                "esc aped"
            ]
        );
        assert_eq!(split_command_line("  "), Vec::<String>::new());
        assert_eq!(split_command_line(r#"a "" b"#), vec!["a", "", "b"]);
        let agent = custom_agent("/opt/bin/agent acp").unwrap();
        assert_eq!(agent.name, "agent");
        assert_eq!(command_line(&agent), "/opt/bin/agent acp");
        assert!(custom_agent("").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn detects_installed_presets() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        for name in ["npx", "omp", "pi", "notes.txt"] {
            let file = directory.path().join(name);
            std::fs::write(&file, "#!/bin/sh\n").unwrap();
            let mode = if name.ends_with(".txt") { 0o644 } else { 0o755 };
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode))
                .unwrap();
        }
        let path = std::env::join_paths([directory.path()]).unwrap();
        let ids: Vec<String> = detect_agents_in(&path)
            .into_iter()
            .map(|agent| agent.id)
            .collect();
        // Codex needs `codex` besides `npx`; gemini, opencode and goose are not installed.
        assert_eq!(ids, vec!["claude", "omp", "pi"]);
        assert!(find_on_path("notes.txt", &path).is_none());
    }
}
