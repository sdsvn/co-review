//! In-app setup (cli spec, "In-app setup"): **Install Agent Skills…**, **Install the co-review Command** and **Add
//! Co-Review to an Agent Harness (MCP)…**. The work is `review_cli`'s `agent_setup` (the same code as the command);
//! this module asks in the palette, runs it off the UI thread and reports the result.

use std::path::PathBuf;

use floem::views::editor::text::SystemClipboard;
use lapce_core::register::Clipboard;
use review_cli::agent_setup::{AgentSetup, HarnessKind, HarnessSetup};

use super::{
    commands::ReviewTab,
    picker::{Choice, Picker},
};

/// The setup for this app: the files that ship with it and its bundled `co-review` command.
fn agent_setup() -> AgentSetup {
    let app = std::env::current_exe().unwrap_or_default();
    AgentSetup::for_app(&app)
}

/// Install Agent Skills…: copies the bundled skills into the chosen agent's skills directory.
pub fn install_skills(tab: &ReviewTab) {
    let setup = agent_setup();
    if setup.skills_directory().is_none() {
        tab.info("This build does not include the agent skills.");
        return;
    }
    let root = tab.review.root.clone();
    let choices = setup
        .skill_targets(root.as_deref())
        .into_iter()
        .map(|target| {
            let (tab, root, id) = (tab.clone(), root.clone(), target.id.clone());
            let directory = target.directory.display().to_string();
            let shown = directory.clone();
            Choice::new(target.label, move || {
                let tab_then = tab.clone();
                tab.later(
                    move || {
                        agent_setup()
                            .install_skills(&id, root.as_deref())
                            .map_err(|error| format!("{error:#}"))
                    },
                    move |result| match result {
                        Ok(installed) => tab_then.info(format!(
                            "Installed {} into {shown}. Agents pick them up in new sessions.",
                            names(&installed)
                        )),
                        Err(error) => {
                            tab_then.error(format!("Could not install the skills: {error}"))
                        }
                    },
                );
            })
            .described(directory)
        })
        .collect();
    tab.pick(Picker::choose(
        "Install the co-review and co-review-design skills into…",
        choices,
    ));
}

fn names(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .filter_map(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Install the co-review Command: links the bundled command into PATH (`bin/install-cli.sh`); `then` hears whether it
/// worked.
pub fn install_command(tab: &ReviewTab, then: impl FnOnce(bool) + 'static) {
    let tab_then = tab.clone();
    tab.later(
        || agent_setup().install_command().map_err(|error| format!("{error:#}")),
        move |result| match result {
            Ok(output) => {
                tab_then.info(if output.is_empty() {
                    "Installed the co-review command.".to_string()
                } else {
                    output
                });
                then(true);
            }
            Err(error) => {
                tab_then.error(format!("Could not install the co-review command: {error}"));
                then(false);
            }
        },
    );
}

/// Add Co-Review to an Agent Harness (MCP)…: a harness whose config can be edited safely gets it written (with a
/// backup) or its commands run, after a confirmation; the others get the snippet copied, to paste in.
pub fn add_to_harness(tab: &ReviewTab) {
    let tab_then = tab.clone();
    tab.later(
        || {
            let setup = agent_setup();
            let harnesses = setup.harnesses(&setup.launch());
            harnesses
                .into_iter()
                .map(|harness| {
                    let needs_command = setup.needs_command(&harness.id);
                    (harness, needs_command)
                })
                .collect::<Vec<_>>()
        },
        move |harnesses| pick_harness(&tab_then, harnesses),
    );
}

fn pick_harness(tab: &ReviewTab, harnesses: Vec<(HarnessSetup, bool)>) {
    let choices = harnesses
        .into_iter()
        .map(|(harness, needs_command)| {
            let description = harness_description(&harness);
            let label = harness.label.clone();
            let tab = tab.clone();
            Choice::new(label, move || {
                if harness.kind == HarnessKind::Manual {
                    copy(&tab, &harness, None);
                } else if needs_command {
                    offer_command(&tab, harness);
                } else {
                    confirm(&tab, harness);
                }
            })
            .described(description)
        })
        .collect();
    tab.pick(Picker::choose("Add Co-Review's MCP server to…", choices));
}

/// What choosing a harness does, shown beside it.
pub fn harness_description(harness: &HarnessSetup) -> String {
    let file = harness
        .file
        .as_ref()
        .map(|file| file.display().to_string())
        .unwrap_or_default();
    match &harness.kind {
        HarnessKind::Cli { commands } => commands
            .iter()
            .map(|command| command.iter().take(3).cloned().collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join(" · "),
        HarnessKind::Json { .. } | HarnessKind::Toml => file,
        HarnessKind::Manual => format!("copies the config to paste into {file}"),
    }
}

/// The Claude Code plugin and the Pi / Oh My Pi packages launch `co-review` from PATH: offer to install it first.
fn offer_command(tab: &ReviewTab, harness: HarnessSetup) {
    let label = harness.label.clone();
    let (install_tab, skip_tab) = (tab.clone(), tab.clone());
    let skipped = harness.clone();
    tab.pick(Picker::choose(
        "This needs the co-review command on your PATH. Install it now?",
        vec![
            Choice::new("Install", move || {
                let tab = install_tab.clone();
                install_command(&install_tab, move |installed| {
                    if installed {
                        confirm(&tab, harness);
                    }
                });
            })
            .described(format!("then add Co-Review to {label}")),
            Choice::new("Skip", move || confirm(&skip_tab, skipped)),
        ],
    ));
}

fn confirm(tab: &ReviewTab, harness: HarnessSetup) {
    let what = match (&harness.kind, &harness.file) {
        (HarnessKind::Cli { .. }, _) => format!("runs: {}", harness.snippet.replace('\n', " && ")),
        (_, Some(file)) => format!(
            "writes {} (the previous version is kept as {}.co-review.bak)",
            file.display(),
            file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
        ),
        (_, None) => String::new(),
    };
    let (add_tab, copy_tab) = (tab.clone(), tab.clone());
    let copied = harness.clone();
    tab.pick(Picker::choose(
        "Add Co-Review to this harness?",
        vec![
            Choice::new(format!("Add to {}", harness.label), move || apply(&add_tab, harness))
                .described(what),
            Choice::new("Copy config", move || copy(&copy_tab, &copied, None))
                .described("to paste in yourself"),
            Choice::new("Cancel", || {}),
        ],
    ));
}

fn apply(tab: &ReviewTab, harness: HarnessSetup) {
    let tab_then = tab.clone();
    let id = harness.id.clone();
    tab.later(
        move || agent_setup().apply(&id).map_err(|error| format!("{error:#}")),
        move |result| match result {
            Ok(done) => tab_then.info(format!("{done} Restart {} to load it.", harness.label)),
            // Whatever went wrong, the snippet still sets it up by hand.
            Err(error) => copy(&tab_then, &harness, Some(error)),
        },
    );
}

fn copy(tab: &ReviewTab, harness: &HarnessSetup, why: Option<String>) {
    let mut clipboard = SystemClipboard::new();
    clipboard.put_string(&harness.snippet);
    let why = why.map(|why| format!("{}. ", why.trim_end_matches('.'))).unwrap_or_default();
    let into = harness
        .file
        .as_ref()
        .map(|file| format!("; paste it into {}", file.display()))
        .unwrap_or_default();
    tab.info(format!("{why}Copied the {} config{into}.", harness.label));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn harness(kind: HarnessKind, file: Option<&str>) -> HarnessSetup {
        HarnessSetup {
            id: "id".to_string(),
            label: "Label".to_string(),
            kind,
            file: file.map(PathBuf::from),
            snippet: String::new(),
        }
    }

    #[test]
    fn harnesses_say_what_choosing_them_does() {
        let commands = vec![
            ["claude", "plugin", "marketplace", "add", "/app"].map(str::to_string).to_vec(),
            ["claude", "plugin", "install", "co-review@co-review"].map(str::to_string).to_vec(),
        ];
        assert_eq!(
            harness_description(&harness(HarnessKind::Cli { commands }, None)),
            "claude plugin marketplace · claude plugin install"
        );
        assert_eq!(
            harness_description(&harness(HarnessKind::Toml, Some("/home/me/.codex/config.toml"))),
            "/home/me/.codex/config.toml"
        );
        assert_eq!(
            harness_description(&harness(HarnessKind::Manual, Some("/home/me/.config/zed/settings.json"))),
            "copies the config to paste into /home/me/.config/zed/settings.json"
        );
    }
}
