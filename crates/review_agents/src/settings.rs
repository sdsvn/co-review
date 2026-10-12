//! The agent's model and reasoning-effort choices (ACP `select` config options).

use std::collections::HashMap;

use agent_client_protocol::schema::v1::{
    SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelectOptions,
};
use serde::{Deserialize, Serialize};

/// A choice an ACP agent offers for its sessions, as the reviewer picks it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSetting {
    /// The ACP config option id, the key in `review.agent.settings`.
    pub id: String,
    pub name: String,
    /// `model` or `thought_level`.
    pub category: String,
    /// The reviewer's choice when there is one, else the agent's default.
    pub current: String,
    pub options: Vec<AgentSettingOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSettingOption {
    pub value: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Categories offered, in the order the reviewer picks them. Modes are not offered: some grant permissions.
const OFFERED: [&str; 2] = ["model", "thought_level"];

fn category_name(category: &SessionConfigOptionCategory) -> String {
    serde_json::to_value(category)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The model and reasoning-effort options among a session's config options (those with a real choice), model
/// first; `chosen` (the review's stored settings) overrides each option's current value.
pub fn offered_settings(
    options: &[SessionConfigOption],
    chosen: Option<&HashMap<String, String>>,
) -> Vec<AgentSetting> {
    let mut settings: Vec<AgentSetting> = options
        .iter()
        .filter_map(|option| {
            let SessionConfigKind::Select(select) = &option.kind else {
                return None;
            };
            let category = option.category.as_ref().map(category_name)?;
            if !OFFERED.contains(&category.as_str()) {
                return None;
            }
            let values = match &select.options {
                SessionConfigSelectOptions::Ungrouped(values) => values.clone(),
                SessionConfigSelectOptions::Grouped(groups) => groups
                    .iter()
                    .flat_map(|group| group.options.clone())
                    .collect(),
                _ => Vec::new(),
            };
            let id = option.id.0.to_string();
            let current = chosen
                .and_then(|chosen| chosen.get(&id).cloned())
                .unwrap_or_else(|| select.current_value.0.to_string());
            Some(AgentSetting {
                id,
                name: option.name.clone(),
                category,
                current,
                options: values
                    .into_iter()
                    .map(|value| AgentSettingOption {
                        value: value.value.0.to_string(),
                        name: value.name,
                        description: value.description,
                    })
                    .collect(),
            })
        })
        .filter(|setting| setting.options.len() > 1)
        .collect();
    settings.sort_by_key(|setting| {
        OFFERED
            .iter()
            .position(|category| *category == setting.category)
    });
    settings
}

/// How the chosen settings read next to the agent, e.g. "Opus · High".
pub fn settings_label(
    offered: &[AgentSetting],
    chosen: &HashMap<String, String>,
) -> String {
    offered
        .iter()
        .filter_map(|setting| {
            let value = chosen.get(&setting.id)?;
            Some(
                setting
                    .options
                    .iter()
                    .find(|option| &option.value == value)
                    .map_or_else(|| value.clone(), |option| option.name.clone()),
            )
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The current value of a session's `select` option.
pub(crate) fn current_value(
    options: &[SessionConfigOption],
    id: &str,
) -> Option<String> {
    options.iter().find(|option| *option.id.0 == *id).and_then(
        |option| match &option.kind {
            SessionConfigKind::Select(select) => {
                Some(select.current_value.0.to_string())
            }
            _ => None,
        },
    )
}
