//! `co-review status [dir]`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::app;
use crate::installation::{self, encode_uri_component, normalize, read_json};

/// An agent integration as its harness recorded it.
#[derive(Debug, PartialEq, Eq)]
pub struct Integration {
    pub harness: String,
    pub version: Option<String>,
    pub location: Option<PathBuf>,
    /// The command that brings it to the app's version.
    pub fix: String,
}

/// Where the harnesses keep their records.
pub struct HarnessDirectories {
    pub home: PathBuf,
    pub claude: PathBuf,
    pub pi: PathBuf,
}

impl HarnessDirectories {
    pub fn from_environment() -> Self {
        let home = installation::user_home();
        let from = |variable: &str, default: PathBuf| {
            std::env::var_os(variable)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or(default)
        };
        HarnessDirectories {
            claude: from("CLAUDE_CONFIG_DIR", home.join(".claude")),
            pi: from("PI_CODING_AGENT_DIR", home.join(".pi/agent")),
            home,
        }
    }
}

/// The Co-Review integrations each agent harness has installed, read from the harness's own records.
/// `bundled_pi` is the Pi package this Co-Review ships.
pub fn integrations(
    directories: &HarnessDirectories,
    bundled_pi: Option<&Path>,
) -> Vec<Integration> {
    let mut found = Vec::new();
    // Claude Code: the plugin from the co-review marketplace, per scope it is installed in.
    let claude_plugins = read_json(&directories.claude.join("plugins/installed_plugins.json"));
    if let Some(installs) = claude_plugins
        .as_ref()
        .and_then(|plugins| plugins.pointer("/plugins/co-review@co-review"))
        .and_then(Value::as_array)
    {
        for install in installs {
            let scope = install
                .get("scope")
                .map(display_value)
                .unwrap_or_else(|| "undefined".to_string());
            found.push(Integration {
                harness: format!("Claude Code plugin ({scope})"),
                version: install.get("version").and_then(Value::as_str).map(str::to_string),
                location: None,
                fix: "claude plugin marketplace update co-review && claude plugin update co-review@co-review"
                    .to_string(),
            });
        }
    }
    // Oh My Pi: the version omp recorded when the package was installed (what `omp plugin list` shows).
    if let Some(omp) = read_json(&directories.home.join(".omp/plugins/omp-plugins.lock.json"))
        .and_then(|lock| lock.pointer("/plugins/co-review-omp").cloned())
    {
        found.push(Integration {
            harness: "Oh My Pi package".to_string(),
            version: omp
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string),
            location: None,
            fix: "co-review setup omp".to_string(),
        });
    }
    // Pi: packages are paths (relative to the agent dir) read live, so the version is that path's package.json.
    let packages = read_json(&directories.pi.join("settings.json"))
        .and_then(|settings| settings.get("packages").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    for entry in packages {
        let source = match &entry {
            Value::String(source) => source.clone(),
            Value::Object(object) => match object.get("source").and_then(Value::as_str) {
                Some(source) => source.to_string(),
                None => continue,
            },
            _ => continue,
        };
        if ["npm:", "git:", "http:", "https:"]
            .iter()
            .any(|scheme| source.starts_with(scheme))
        {
            continue;
        }
        let expanded = match source.strip_prefix("~/") {
            Some(rest) => directories.home.join(rest),
            None => PathBuf::from(&source),
        };
        let directory = normalize(&directories.pi.join(expanded));
        let Some(package) = read_json(&directory.join("package.json")) else {
            continue;
        };
        if package.get("name").and_then(Value::as_str) != Some("co-review-pi") {
            continue;
        }
        let fix = if bundled_pi.is_some_and(|bundled| normalize(bundled) == directory) {
            "update Co-Review (the package it ships is out of date)".to_string()
        } else {
            format!("pi remove {source} && co-review setup pi")
        };
        found.push(Integration {
            harness: "Pi package".to_string(),
            version: package
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string),
            location: Some(directory),
            fix,
        });
    }
    found
}

fn display_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The version lines: this Co-Review's, then each integration's, flagging any that differ.
pub fn version_report(app_version: &str, found: &[Integration]) -> Vec<String> {
    let mut lines = vec![format!("co-review {app_version}")];
    for integration in found {
        let location = integration
            .location
            .as_ref()
            .map(|location| format!(" ({})", location.display()))
            .unwrap_or_default();
        lines.push(match &integration.version {
            Some(version) if version == app_version => {
                format!("  {}: {version}{location}", integration.harness)
            }
            version => format!(
                "  {}: {}{location}  ← differs from the app; update it with: {}",
                integration.harness,
                version.as_deref().unwrap_or("unknown"),
                integration.fix
            ),
        });
    }
    if found.is_empty() {
        lines.push(
            "  no agent integrations installed (see https://sdsvn.github.io/co-review/#setup)"
                .to_string(),
        );
    }
    lines
}

/// This Co-Review's version and each installed integration's (flagging any that differ), then the open reviews of
/// `root` from a running Co-Review only: never starts it. With `claude_hook`, only the reviews (for the hook).
pub fn run(root: &Path, claude_hook: bool) {
    if !claude_hook {
        let bundled_pi = installation::distribution_root().map(|root| root.join("integrations/pi"));
        let found = integrations(
            &HarnessDirectories::from_environment(),
            bundled_pi.as_deref(),
        );
        for line in version_report(&installation::version(), &found) {
            println!("{line}");
        }
    }
    if let Some(url) =
        app::server_record(&installation::co_review_home()).and_then(|server| server.url)
    {
        let address = format!(
            "{url}/api/m/status?root={}{}",
            encode_uri_component(&root.to_string_lossy()),
            if claude_hook {
                "&format=claude-hook"
            } else {
                ""
            }
        );
        let response = app::http_agent()
            .get(&address)
            .timeout(Duration::from_millis(1500))
            .call();
        if let Ok(response) = response
            && response.status() == 200
            && let Ok(text) = response.into_string()
        {
            if claude_hook {
                println!("{text}");
                return;
            }
            if let Ok(reviews) = serde_json::from_str::<Value>(&text) {
                let pretty = serde_json::to_string_pretty(&reviews).unwrap_or(text);
                println!("Open reviews of {}:\n{pretty}", root.display());
                return;
            }
        }
    }
    if !claude_hook {
        println!("Co-Review is not running.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn integrations_of_another_version_are_flagged_with_their_fix() {
        let home = tempfile::tempdir().expect("temporary directory");
        let directories = HarnessDirectories {
            home: home.path().to_path_buf(),
            claude: home.path().join(".claude"),
            pi: home.path().join(".pi/agent"),
        };
        fs::create_dir_all(directories.claude.join("plugins")).expect("directory");
        fs::write(
            directories.claude.join("plugins/installed_plugins.json"),
            r#"{"plugins":{"co-review@co-review":[{"scope":"user","version":"0.6.2"},{"scope":"project","version":"0.6.3"}]}}"#,
        )
        .expect("write");
        let bundled = home.path().join("app/integrations/pi");
        fs::create_dir_all(&bundled).expect("directory");
        fs::write(
            bundled.join("package.json"),
            r#"{"name":"co-review-pi","version":"0.6.1"}"#,
        )
        .expect("write");
        fs::create_dir_all(&directories.pi).expect("directory");
        fs::write(
            directories.pi.join("settings.json"),
            r#"{"packages":["npm:other","../../app/integrations/pi"]}"#,
        )
        .expect("write");

        let found = integrations(&directories, Some(&bundled));
        let report = version_report("0.6.3", &found);
        assert_eq!(report[0], "co-review 0.6.3");
        assert_eq!(
            report[1],
            "  Claude Code plugin (user): 0.6.2  ← differs from the app; update it with: \
             claude plugin marketplace update co-review && claude plugin update co-review@co-review"
        );
        assert_eq!(report[2], "  Claude Code plugin (project): 0.6.3");
        assert_eq!(
            report[3],
            format!(
                "  Pi package: 0.6.1 ({})  ← differs from the app; update it with: update Co-Review (the package it ships is out of date)",
                bundled.display()
            )
        );
        assert_eq!(report.len(), 4);
    }

    #[test]
    fn no_integrations_points_to_setup() {
        assert_eq!(
            version_report("0.6.3", &[]),
            vec![
                "co-review 0.6.3",
                "  no agent integrations installed (see https://sdsvn.github.io/co-review/#setup)"
            ]
        );
    }
}
