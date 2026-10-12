//! Connecting agents to Co-Review from inside the app: installs the bundled skills into an agent's skills directory,
//! installs the `co-review` command, and adds Co-Review's MCP server to an agent harness's config (or hands out the
//! snippet where the config format isn't safe to edit).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::installation;

const SERVER: &str = "co-review";

/// How a harness launches `co-review mcp`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpLaunch {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillTarget {
    pub id: String,
    pub label: String,
    pub directory: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HarnessKind {
    /// Set up by running these commands.
    Cli { commands: Vec<Vec<String>> },
    /// A JSON config: the snippet's entry is merged under `key`.
    Json { key: String },
    /// A TOML config: the snippet is appended.
    Toml,
    /// A config not safe to edit: the snippet is pasted by hand.
    Manual,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HarnessSetup {
    pub id: String,
    pub label: String,
    pub kind: HarnessKind,
    pub file: Option<PathBuf>,
    pub snippet: String,
}

pub struct AgentSetup {
    /// Where the files that ship with Co-Review are (`plugin/`, `integrations/`, `bin/`).
    pub distribution: Option<PathBuf>,
    pub home: PathBuf,
    /// Where an installed `co-review` command may be.
    pub command_locations: Vec<PathBuf>,
    /// This command's own executable, for harnesses when the command is not installed.
    pub executable: PathBuf,
    pub mac: bool,
}

impl AgentSetup {
    pub fn new() -> Self {
        let home = installation::user_home();
        AgentSetup {
            distribution: installation::distribution_root(),
            command_locations: vec![
                PathBuf::from("/usr/local/bin").join(SERVER),
                home.join(".local/bin").join(SERVER),
            ],
            executable: std::env::current_exe().unwrap_or_else(|_| PathBuf::from(SERVER)),
            mac: cfg!(target_os = "macos"),
            home,
        }
    }

    fn bundled(&self, relative: &str) -> Option<PathBuf> {
        let path = self.distribution.as_ref()?.join(relative);
        path.exists().then_some(path)
    }

    pub fn skills_directory(&self) -> Option<PathBuf> {
        self.bundled("plugin/skills")
    }

    /// Root with `.claude-plugin/marketplace.json`.
    pub fn marketplace_directory(&self) -> Option<PathBuf> {
        self.bundled(".claude-plugin/marketplace.json")
            .and(self.distribution.clone())
    }

    pub fn installed_command(&self) -> Option<PathBuf> {
        self.command_locations
            .iter()
            .find(|path| path.exists())
            .cloned()
    }

    /// Installs the `co-review` commands (`bin/install-cli.sh`: /usr/local/bin when writable, else ~/.local/bin).
    pub fn install_command(&self) -> Result<String> {
        let script = self
            .bundled("bin/install-cli.sh")
            .context("install-cli.sh is not bundled with this build.")?;
        execute("/bin/sh", &[script.to_string_lossy().to_string()])
    }

    /// How a harness should launch `co-review mcp`: the installed command, else this executable.
    pub fn launch(&self) -> McpLaunch {
        let command = self
            .installed_command()
            .unwrap_or_else(|| self.executable.clone());
        McpLaunch {
            command: command.to_string_lossy().to_string(),
            args: vec!["mcp".to_string()],
            env: BTreeMap::new(),
        }
    }

    pub fn skill_targets(&self, workspace_root: Option<&Path>) -> Vec<SkillTarget> {
        let mut targets = vec![
            SkillTarget {
                id: "claude".to_string(),
                label: "Claude Code (user)".to_string(),
                directory: self.home.join(".claude/skills"),
            },
            SkillTarget {
                id: "agents".to_string(),
                label: "Shared agents directory (Pi, skills CLI)".to_string(),
                directory: self.home.join(".agents/skills"),
            },
        ];
        if let Some(root) = workspace_root {
            targets.push(SkillTarget {
                id: "project".to_string(),
                label: "This repository (Claude Code)".to_string(),
                directory: root.join(".claude/skills"),
            });
        }
        targets
    }

    /// Copies every bundled skill (a directory with a `SKILL.md`) into the target, replacing what was there.
    pub fn install_skills(
        &self,
        target_id: &str,
        workspace_root: Option<&Path>,
    ) -> Result<Vec<PathBuf>> {
        let source = self
            .skills_directory()
            .context("The skills are not bundled with this build.")?;
        let target = self
            .skill_targets(workspace_root)
            .into_iter()
            .find(|target| target.id == target_id)
            .with_context(|| format!("Unknown target {target_id}"))?;
        let mut names: Vec<_> = std::fs::read_dir(&source)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().join("SKILL.md").is_file())
            .map(|entry| entry.file_name())
            .collect();
        names.sort();
        let mut installed = Vec::new();
        for name in names {
            let destination = target.directory.join(&name);
            if destination.exists() {
                std::fs::remove_dir_all(&destination)?;
            }
            copy_directory(&source.join(&name), &destination)?;
            installed.push(destination);
        }
        Ok(installed)
    }

    pub fn harnesses(&self, launch: &McpLaunch) -> Vec<HarnessSetup> {
        let home = &self.home;
        let has_env = !launch.env.is_empty();
        let mut stdio = Map::new();
        stdio.insert("command".to_string(), json!(launch.command));
        stdio.insert("args".to_string(), json!(launch.args));
        if has_env {
            stdio.insert("env".to_string(), json!(launch.env));
        }
        let with = |extra: Value| {
            let mut entry = stdio.clone();
            if let Value::Object(extra) = extra {
                entry.extend(extra);
            }
            Value::Object(entry)
        };
        let pretty = |value: Value| serde_json::to_string_pretty(&value).unwrap_or_default();
        let application_support = if self.mac {
            home.join("Library/Application Support")
        } else {
            home.join(".config")
        };
        let mut claude_arguments: Vec<String> =
            ["mcp", "add", "-s", "user"].map(str::to_string).to_vec();
        for (key, value) in &launch.env {
            claude_arguments.push("-e".to_string());
            claude_arguments.push(format!("{key}={value}"));
        }
        claude_arguments.extend([SERVER.to_string(), "--".to_string(), launch.command.clone()]);
        claude_arguments.extend(launch.args.iter().cloned());
        let cli = |label: &str, id: &str, commands: Vec<Vec<String>>| HarnessSetup {
            id: id.to_string(),
            label: label.to_string(),
            snippet: commands
                .iter()
                .map(|command| {
                    command
                        .iter()
                        .map(|word| shell_quote(word))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect::<Vec<_>>()
                .join("\n"),
            kind: HarnessKind::Cli { commands },
            file: None,
        };
        let json_harness =
            |id: &str, label: &str, file: PathBuf, key: &str, entry: Value| HarnessSetup {
                id: id.to_string(),
                label: label.to_string(),
                kind: HarnessKind::Json {
                    key: key.to_string(),
                },
                file: Some(file),
                snippet: pretty(json!({ key: { SERVER: entry } })),
            };
        let words = |words: &[&str]| {
            words
                .iter()
                .map(|word| word.to_string())
                .collect::<Vec<_>>()
        };

        let mut harnesses = Vec::new();
        if let Some(marketplace) = self.marketplace_directory() {
            harnesses.push(cli(
                "Claude Code: plugin (skills, commands and MCP server)",
                "claude-code-plugin",
                vec![
                    words(&[
                        "claude",
                        "plugin",
                        "marketplace",
                        "add",
                        &marketplace.to_string_lossy(),
                    ]),
                    words(&["claude", "plugin", "install", "co-review@co-review"]),
                ],
            ));
        }
        let mut claude = vec!["claude".to_string()];
        claude.extend(claude_arguments);
        harnesses.push(cli(
            "Claude Code: MCP server only",
            "claude-code",
            vec![claude],
        ));
        if let Some(pi) = self.bundled("integrations/pi") {
            harnesses.push(cli(
                "Pi: package (extension, /co-review, co-reviewer subagent, skills)",
                "pi",
                vec![words(&["pi", "install", &pi.to_string_lossy()])],
            ));
        }
        if let Some(omp) = self.bundled("integrations/omp") {
            harnesses.push(cli(
                "Oh My Pi: package (extension, /co-review, /co-review-design, skills)",
                "omp",
                vec![words(&["omp", "install", &omp.to_string_lossy()])],
            ));
        }
        let mut codex = vec![
            format!("[mcp_servers.{SERVER}]"),
            format!("command = {}", json!(launch.command)),
            format!("args = {}", json!(launch.args)),
        ];
        if has_env {
            let pairs: Vec<String> = launch
                .env
                .iter()
                .map(|(key, value)| format!("{key} = {}", json!(value)))
                .collect();
            codex.push(format!("env = {{ {} }}", pairs.join(", ")));
        }
        codex.push("# await_reviewer and ask_reviewer block for minutes".to_string());
        codex.push("tool_timeout_sec = 600".to_string());
        harnesses.push(HarnessSetup {
            id: "codex".to_string(),
            label: "Codex".to_string(),
            kind: HarnessKind::Toml,
            file: Some(home.join(".codex/config.toml")),
            snippet: codex.join("\n"),
        });
        harnesses.push(json_harness(
            "cursor",
            "Cursor",
            home.join(".cursor/mcp.json"),
            "mcpServers",
            with(json!({})),
        ));
        harnesses.push(json_harness(
            "claude-desktop",
            "Claude Desktop",
            application_support.join("Claude/claude_desktop_config.json"),
            "mcpServers",
            with(json!({})),
        ));
        let mut vscode = Map::new();
        vscode.insert("type".to_string(), json!("stdio"));
        vscode.extend(stdio.clone());
        harnesses.push(json_harness(
            "vscode",
            "VS Code (Copilot)",
            application_support.join("Code/User/mcp.json"),
            "servers",
            Value::Object(vscode),
        ));
        harnesses.push(json_harness(
            "windsurf",
            "Windsurf",
            home.join(".codeium/windsurf/mcp_config.json"),
            "mcpServers",
            with(json!({})),
        ));
        harnesses.push(json_harness(
            "gemini",
            "Gemini CLI",
            home.join(".gemini/settings.json"),
            "mcpServers",
            with(json!({ "timeout": 600000 })),
        ));
        let mut command = vec![launch.command.clone()];
        command.extend(launch.args.iter().cloned());
        let mut opencode = Map::new();
        opencode.insert("type".to_string(), json!("local"));
        opencode.insert("command".to_string(), json!(command));
        opencode.insert("enabled".to_string(), json!(true));
        if has_env {
            opencode.insert("environment".to_string(), json!(launch.env));
        }
        harnesses.push(json_harness(
            "opencode",
            "OpenCode",
            home.join(".config/opencode/opencode.json"),
            "mcp",
            Value::Object(opencode),
        ));
        let mut zed = Map::new();
        zed.insert("source".to_string(), json!("custom"));
        zed.extend(stdio.clone());
        harnesses.push(HarnessSetup {
            id: "zed".to_string(),
            label: "Zed".to_string(),
            kind: HarnessKind::Manual,
            file: Some(home.join(".config/zed/settings.json")),
            snippet: pretty(json!({ "context_servers": { SERVER: zed } })),
        });
        let mut goose = vec![
            "extensions:".to_string(),
            format!("  {SERVER}:"),
            format!("    name: {SERVER}"),
            "    type: stdio".to_string(),
            format!("    cmd: {}", json!(launch.command)),
            format!("    args: {}", json!(launch.args)),
        ];
        if has_env {
            goose.push(format!("    envs: {}", json!(launch.env)));
        }
        goose.extend([
            "    enabled: true".to_string(),
            "    timeout: 600".to_string(),
        ]);
        harnesses.push(HarnessSetup {
            id: "goose".to_string(),
            label: "Goose".to_string(),
            kind: HarnessKind::Manual,
            file: Some(home.join(".config/goose/config.yaml")),
            snippet: goose.join("\n"),
        });
        harnesses
    }

    /// Adds Co-Review to the harness's config; returns a one-line result. JSON is merged, never overwritten.
    pub fn apply(&self, harness_id: &str) -> Result<String> {
        let harness = self
            .harnesses(&self.launch())
            .into_iter()
            .find(|harness| harness.id == harness_id)
            .with_context(|| format!("Unknown harness {harness_id}"))?;
        let file = match (&harness.kind, &harness.file) {
            (HarnessKind::Cli { commands }, _) => {
                for command in commands {
                    if let Some((program, arguments)) = command.split_first() {
                        execute(program, arguments)?;
                    }
                }
                return Ok(format!("Set up {}.", harness.label));
            }
            (HarnessKind::Manual, file) | (_, file @ None) => {
                let file = file
                    .as_ref()
                    .map(|file| file.display().to_string())
                    .unwrap_or_default();
                bail!(
                    "{}'s config can't be edited safely; paste the snippet into {file}.",
                    harness.label
                );
            }
            (_, Some(file)) => file,
        };
        if let Some(directory) = file.parent() {
            std::fs::create_dir_all(directory)?;
        }
        let existing = std::fs::read_to_string(file).unwrap_or_default();
        if harness.kind == HarnessKind::Toml {
            if existing.contains(&format!("[mcp_servers.{SERVER}]")) {
                return Ok(format!(
                    "{} already has [mcp_servers.{SERVER}]; left unchanged.",
                    file.display()
                ));
            }
            let separator = if !existing.is_empty() && !existing.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            std::fs::write(
                file,
                format!("{existing}{separator}\n{}\n", harness.snippet),
            )?;
            return Ok(format!("Added to {}.", file.display()));
        }
        let HarnessKind::Json { key } = &harness.kind else {
            bail!(
                "{}'s config can't be edited safely; paste the snippet into {}.",
                harness.label,
                file.display()
            );
        };
        let mut config: Map<String, Value> = if existing.trim().is_empty() {
            Map::new()
        } else {
            serde_json::from_str(&existing).map_err(|_| {
                anyhow::anyhow!(
                    "{} isn't plain JSON (comments?); paste the snippet instead.",
                    file.display()
                )
            })?
        };
        let snippet: Value = serde_json::from_str(&harness.snippet)?;
        let entry = snippet
            .pointer(&format!("/{key}/{SERVER}"))
            .cloned()
            .context("the snippet has no entry")?;
        let mut servers = match config.remove(key) {
            Some(Value::Object(servers)) => servers,
            _ => Map::new(),
        };
        servers.insert(SERVER.to_string(), entry);
        config.insert(key.clone(), Value::Object(servers));
        let backup = PathBuf::from(format!("{}.co-review.bak", file.display()));
        if !existing.is_empty() {
            std::fs::write(&backup, &existing)?;
        }
        std::fs::write(
            file,
            serde_json::to_string_pretty(&Value::Object(config))? + "\n",
        )?;
        if existing.is_empty() {
            Ok(format!("Added to {}.", file.display()))
        } else {
            let name = backup
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            Ok(format!(
                "Added to {} (previous version saved as {name}).",
                file.display()
            ))
        }
    }
}

impl Default for AgentSetup {
    fn default() -> Self {
        Self::new()
    }
}

/// A word as a POSIX shell reads it.
fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || "@%+=:,./-".contains(character)
        });
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

fn execute(program: &str, arguments: &[String]) -> Result<String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .with_context(|| format!("could not run {program}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if error.is_empty() {
        bail!("{program} failed ({})", output.status);
    }
    bail!("{error}")
}

fn copy_directory(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup(home: &Path) -> AgentSetup {
        AgentSetup {
            distribution: None,
            home: home.to_path_buf(),
            command_locations: vec![home.join("bin/co-review")],
            executable: PathBuf::from("/opt/co review/co-review"),
            mac: true,
        }
    }

    #[test]
    fn snippets_launch_the_command() {
        let home = tempfile::tempdir().expect("temporary directory");
        let setup = setup(home.path());
        let harnesses = setup.harnesses(&setup.launch());
        let snippet = |id: &str| {
            harnesses
                .iter()
                .find(|harness| harness.id == id)
                .map(|harness| harness.snippet.clone())
                .unwrap_or_default()
        };
        assert_eq!(
            snippet("claude-code"),
            "claude mcp add -s user co-review -- '/opt/co review/co-review' mcp"
        );
        assert_eq!(
            snippet("codex"),
            "[mcp_servers.co-review]\ncommand = \"/opt/co review/co-review\"\nargs = [\"mcp\"]\n\
             # await_reviewer and ask_reviewer block for minutes\ntool_timeout_sec = 600"
        );
        assert_eq!(
            snippet("vscode"),
            "{\n  \"servers\": {\n    \"co-review\": {\n      \"type\": \"stdio\",\n      \"command\": \"/opt/co review/co-review\",\n      \"args\": [\n        \"mcp\"\n      ]\n    }\n  }\n}"
        );
    }

    #[test]
    fn json_configs_are_merged_and_backed_up() {
        let home = tempfile::tempdir().expect("temporary directory");
        let file = home.path().join(".cursor/mcp.json");
        fs::create_dir_all(file.parent().expect("parent")).expect("directory");
        fs::write(
            &file,
            r#"{"mcpServers":{"other":{"command":"x"}},"theme":"dark"}"#,
        )
        .expect("write");
        let result = setup(home.path()).apply("cursor").expect("apply");
        assert!(
            result.ends_with("(previous version saved as mcp.json.co-review.bak)."),
            "{result}"
        );
        let config: Value =
            serde_json::from_str(&fs::read_to_string(&file).expect("read")).expect("json");
        assert_eq!(config["mcpServers"]["other"]["command"], "x");
        assert_eq!(config["mcpServers"]["co-review"]["args"], json!(["mcp"]));
        assert_eq!(config["theme"], "dark");
        assert!(home.path().join(".cursor/mcp.json.co-review.bak").exists());
    }

    #[test]
    fn toml_is_appended_once() {
        let home = tempfile::tempdir().expect("temporary directory");
        let setup = setup(home.path());
        assert!(setup.apply("codex").expect("apply").starts_with("Added to"));
        assert!(
            setup
                .apply("codex")
                .expect("apply")
                .ends_with("left unchanged.")
        );
        let text = fs::read_to_string(home.path().join(".codex/config.toml")).expect("read");
        assert_eq!(text.matches("[mcp_servers.co-review]").count(), 1);
    }

    #[test]
    fn manual_configs_are_refused() {
        let home = tempfile::tempdir().expect("temporary directory");
        let error = setup(home.path()).apply("zed").expect_err("manual");
        assert!(error.to_string().contains("can't be edited safely"));
    }
}
