//! Co-Review Dark and Light following the OS (desktop-app spec). With `core.follow-os-theme` on (the default), a
//! color theme that has a Dark and a Light version (`Co-Review Dark` / `Co-Review Light`, `Lapce Dark` / `Lapce
//! Light`) shows the version matching the OS appearance; the setting keeps the name the reviewer chose. Choosing a
//! theme that doesn't match the OS in the palette turns following off, so the choice sticks.

use std::{
    collections::HashMap,
    sync::atomic::{AtomicU8, Ordering},
};

use floem::window::Theme;

use crate::config::{LapceConfig, core::CoreConfig};

/// The OS appearance the windows last reported: 0 not known yet (no window), 1 light, 2 dark.
static OS_APPEARANCE: AtomicU8 = AtomicU8::new(0);

/// Whether the OS appearance is dark, once a window reported it.
pub fn os_is_dark() -> Option<bool> {
    match OS_APPEARANCE.load(Ordering::Relaxed) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

/// A window reported the OS appearance (`Event::ThemeChanged`, also sent when a window opens); true when it
/// changed, and the config should be reloaded.
pub fn os_theme_changed(theme: Theme) -> bool {
    let appearance = match theme {
        Theme::Dark => 2,
        Theme::Light => 1,
    };
    OS_APPEARANCE.swap(appearance, Ordering::Relaxed) != appearance
}

/// The color theme to show: the configured one, or its other version when it follows the OS.
pub fn effective_color_theme(
    core: &CoreConfig,
    available: &HashMap<String, (String, config::Config)>,
) -> String {
    match os_is_dark() {
        Some(dark) if core.follow_os_theme => {
            following(&core.color_theme, dark, |name| available.contains_key(&name.to_lowercase()))
        }
        _ => core.color_theme.clone(),
    }
}

/// `name`, or its version for the OS appearance when there is one (`X Dark` ↔ `X Light`).
pub fn following(name: &str, dark: bool, exists: impl Fn(&str) -> bool) -> String {
    let (from, to) = if dark { (" Light", " Dark") } else { (" Dark", " Light") };
    name.strip_suffix(from)
        .map(|base| format!("{base}{to}"))
        .filter(|other| exists(other))
        .unwrap_or_else(|| name.to_string())
}

/// A theme was chosen in the palette (and saved): one that the OS appearance would switch away from turns following
/// off, so it stays.
pub fn theme_chosen(name: &str, config: &LapceConfig) {
    let Some(dark) = os_is_dark() else {
        return;
    };
    if !config.core.follow_os_theme {
        return;
    }
    let shown = following(name, dark, |other| {
        config.available_color_themes.contains_key(&other.to_lowercase())
    });
    if shown != name {
        let saved =
            LapceConfig::update_file("core", "follow-os-theme", toml_edit::Value::from(false));
        if saved.is_none() {
            tracing::error!("could not turn off core.follow-os-theme for the theme {name}");
        }
    }
}

/// The colour of agents' messages, questions and activity in threads (`review.agent` in the themes).
pub const AGENT_COLOR: &str = "review.agent";

#[cfg(test)]
mod tests {
    use super::following;

    #[test]
    fn a_theme_pair_follows_the_os() {
        let exists = |name: &str| matches!(name, "Co-Review Dark" | "Co-Review Light");
        assert_eq!(following("Co-Review Dark", false, exists), "Co-Review Light");
        assert_eq!(following("Co-Review Light", true, exists), "Co-Review Dark");
        assert_eq!(following("Co-Review Dark", true, exists), "Co-Review Dark");
        // No other version: the theme stays.
        assert_eq!(following("Solarized Dark", false, exists), "Solarized Dark");
        assert_eq!(following("Monokai", false, exists), "Monokai");
    }

    /// Co-Review's themes set every colour Lapce's own themes do, and the agent colour.
    #[test]
    fn the_themes_set_every_colour() {
        let keys = |source: &str, table: &str| -> Vec<String> {
            let document: toml_edit::Document = source.parse().expect("a theme");
            let mut keys: Vec<String> = document["color-theme"][table]
                .as_table()
                .expect("a table")
                .iter()
                .map(|(key, _)| key.to_string())
                .collect();
            keys.sort();
            keys
        };
        let lapce = include_str!("../../../defaults/dark-theme.toml");
        for theme in [
            include_str!("../../../defaults/co-review-dark-theme.toml"),
            include_str!("../../../defaults/co-review-light-theme.toml"),
        ] {
            for table in ["base", "syntax", "ui"] {
                assert_eq!(keys(theme, table), keys(lapce, table), "{table}");
            }
            assert!(keys(theme, "ui").contains(&super::AGENT_COLOR.to_string()));
        }
    }
}
