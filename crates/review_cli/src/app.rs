//! Finding a running Co-Review, and starting one: the only place that knows how the app is launched.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, bail};
use serde::Deserialize;

use crate::debug_log::{log, log_in};
use crate::installation::{self, co_review_home, logs_directory};
use crate::process;

const START_ATTEMPTS: u32 = 120;
const START_INTERVAL: Duration = Duration::from_millis(500);

/// The Co-Review app this command starts when none is running: started with a repository to open it, or with
/// `--background` for an agent (no window until a review is shown). It records its backend in `server.json`, its
/// process and version in `logs/app.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    pub executable: PathBuf,
    /// The version installed on disk; a running app of another version is stopped before starting.
    pub version: Option<String>,
}

/// The app's executable in a release (packaging/package.sh): `Co-Review.app/Contents/MacOS/Co-Review` on macOS,
/// `<dir>/co-review-app` on Linux, `<dir>\Co-Review.exe` on Windows; in each, this command is
/// `resources/app/bin/co-review` (case-insensitive file systems can't hold `Co-Review` and `co-review` side by side).
const MAC_BUNDLE_EXECUTABLE: &str = "Co-Review.app/Contents/MacOS/Co-Review";
const DIRECTORY_EXECUTABLES: [&str; 2] = ["co-review-app", "Co-Review.exe"];

impl App {
    /// `$CO_REVIEW_APP` (the app's executable, or `none` to only use a running Co-Review), else the app this command
    /// is bundled in, else the app built beside it (`cargo build`), else the installed app.
    pub fn locate() -> Option<App> {
        if let Some(configured) =
            std::env::var_os("CO_REVIEW_APP").filter(|value| !value.is_empty())
        {
            if configured == "none" {
                return None;
            }
            return Some(App::new(PathBuf::from(configured)));
        }
        if let Some(executable) = installation::bundled_resources()
            .and_then(|resources| bundle_executable(&resources))
        {
            return Some(App::new(executable));
        }
        if let Some(executable) = std::env::current_exe()
            .ok()
            .and_then(|command| built_app(command.parent()?))
        {
            return Some(App::new(executable));
        }
        installed_app().map(App::new)
    }

    fn new(executable: PathBuf) -> App {
        let version = installation::resources_of(&executable)
            .and_then(|resources| installation::installed_version(&resources));
        App {
            executable,
            version,
        }
    }
}

/// The app built in the same cargo profile directory as this command (`target/debug/lapce`); only there, so an
/// unrelated `lapce` installed beside the command is never taken for Co-Review.
fn built_app(directory: &Path) -> Option<PathBuf> {
    if !directory.join(".fingerprint").is_dir() {
        return None;
    }
    ["lapce", "lapce.exe"]
        .into_iter()
        .map(|name| directory.join(name))
        .find(|candidate| candidate.is_file())
}

/// The app's executable for its `resources/app` directory.
fn bundle_executable(resources: &Path) -> Option<PathBuf> {
    let contents = resources.parent()?;
    let candidates = if contents.file_name()? == "Resources" {
        // macOS: <Name>.app/Contents/Resources/app → <Name>.app/Contents/MacOS/<Name>
        let bundle = contents.parent()?.parent()?;
        let name = bundle.file_stem()?.to_owned();
        vec![contents.parent()?.join("MacOS").join(name)]
    } else {
        let directory = contents.parent()?;
        DIRECTORY_EXECUTABLES
            .iter()
            .map(|name| directory.join(name))
            .collect()
    };
    candidates.into_iter().find(|candidate| candidate.is_file())
}

/// Where `install.sh` (or the Windows installer) puts the app: `$CO_REVIEW_APPS_DIR`, /Applications or
/// ~/Applications on macOS; `$CO_REVIEW_INSTALL_DIR`, else ~/.local/share/co-review on Linux and
/// %LOCALAPPDATA%\Programs\Co-Review on Windows.
pub fn installed_app() -> Option<PathBuf> {
    let home = installation::user_home();
    let mut candidates = Vec::new();
    if cfg!(target_os = "macos") {
        let mut directories: Vec<PathBuf> = std::env::var_os("CO_REVIEW_APPS_DIR")
            .map(PathBuf::from)
            .into_iter()
            .collect();
        directories.push(PathBuf::from("/Applications"));
        directories.push(home.join("Applications"));
        candidates.extend(
            directories
                .into_iter()
                .map(|directory| directory.join(MAC_BUNDLE_EXECUTABLE)),
        );
    } else {
        let directory = std::env::var_os("CO_REVIEW_INSTALL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    std::env::var_os("LOCALAPPDATA")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| home.join("AppData/Local"))
                        .join("Programs/Co-Review")
                } else {
                    home.join(".local/share/co-review")
                }
            });
        candidates.extend(DIRECTORY_EXECUTABLES.iter().map(|name| directory.join(name)));
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

pub fn http_agent() -> ureq::Agent {
    // No pooled connections: a Co-Review that restarted would leave dead ones behind.
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(3))
        .max_idle_connections(0)
        .build()
}

#[derive(Deserialize, Default)]
pub struct ServerRecord {
    pub url: Option<String>,
    pub pid: Option<u32>,
}

/// `server.json`: the backend that runs, as it recorded itself.
pub fn server_record(home: &Path) -> Option<ServerRecord> {
    let contents = std::fs::read_to_string(home.join("server.json")).ok()?;
    serde_json::from_str(&contents).ok()
}

fn registered(home: &Path) -> Option<String> {
    server_record(home)?.url
}

/// Whether a Co-Review answers at `base` (its `/mcp` endpoint refuses a GET without a session: 400 from the Theia
/// app, 406 from the Lapce app's rmcp server, which wants `Accept: text/event-stream`).
pub fn is_running(base: &str) -> bool {
    match http_agent()
        .get(&format!("{base}/mcp"))
        .timeout(Duration::from_millis(1500))
        .call()
    {
        Ok(_) => true,
        Err(ureq::Error::Status(status, _)) => matches!(status, 400 | 404 | 406),
        Err(ureq::Error::Transport(_)) => false,
    }
}

fn port_base(port: Option<u16>) -> String {
    format!("http://127.0.0.1:{}", port.unwrap_or(3000))
}

/// The base URL of a Co-Review that is already running; never starts one. Without a port, the one recorded in
/// `server.json`. The app picks its own port, so a port is only looked at when there is no app to start (`--port` /
/// `$CO_REVIEW_PORT` with `CO_REVIEW_APP=none`).
pub fn find_server(app: Option<&App>, port: Option<u16>) -> Option<String> {
    if port.is_none()
        && let Some(url) = registered(&co_review_home())
        && is_running(&url)
    {
        return Some(url);
    }
    if app.is_some() {
        return None;
    }
    let base = port_base(port);
    is_running(&base).then_some(base)
}

/// The base URL of a Co-Review, started if none runs. `open` opens `root` in the app (a running one picks it up);
/// otherwise the app starts in the background.
pub fn ensure_server(
    app: Option<&App>,
    port: Option<u16>,
    root: &Path,
    open: bool,
) -> Result<String> {
    if let Some(app) = app {
        // An app of another version, or one left without a backend, is not reused: stopped, and started afresh.
        stop_stale_desktop(&co_review_home(), &StaleOptions::new(app.version.clone()));
        if open {
            return start_app(&app.executable, Some(root));
        }
    }
    if let Some(running) = find_server(app, port) {
        return Ok(running);
    }
    match app {
        Some(app) => start_app(&app.executable, None),
        None => bail!(
            "Co-Review is not running, and no Co-Review app was found to start (install it: \
             https://sdsvn.github.io/co-review/#setup, or set CO_REVIEW_APP to its executable)"
        ),
    }
}

/// Opens `root` in the app, or with no `root` starts it in the background (no window), and waits for its backend to
/// register.
fn start_app(executable: &Path, root: Option<&Path>) -> Result<String> {
    match root {
        Some(root) => log(&format!(
            "starting the Co-Review desktop app for {}",
            root.display()
        )),
        None => log("starting the Co-Review desktop app in the background"),
    }
    let mut command = Command::new(executable);
    match root {
        Some(root) => command.arg(root),
        None => command.arg("--background"),
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    spawn_detached(&mut command)?;
    let home = co_review_home();
    for _ in 0..START_ATTEMPTS {
        if let Some(url) = registered(&home)
            && is_running(&url)
        {
            return Ok(url);
        }
        std::thread::sleep(START_INTERVAL);
    }
    bail!("The Co-Review desktop app did not start.")
}

/// Starts a process that outlives this one (its own process group), reaped here if it exits first.
fn spawn_detached(command: &mut Command) -> Result<()> {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    let mut child = command.spawn()?;
    std::thread::spawn(move || child.wait().ok());
    Ok(())
}

#[derive(Deserialize)]
struct AppRecord {
    pid: Option<u32>,
    version: Option<String>,
    started: Option<String>,
}

pub struct StaleOptions {
    pub now: SystemTime,
    /// How long an app may run without a backend while it starts.
    pub grace: Duration,
    /// The version installed (the one this command would start); None in a checkout, where only the backend counts.
    pub version: Option<String>,
}

impl StaleOptions {
    pub fn new(version: Option<String>) -> Self {
        StaleOptions {
            now: SystemTime::now(),
            grace: Duration::from_secs(90),
            version,
        }
    }
}

/// Stops a running desktop app that must not be used: one of another version than the installed one (an update was
/// installed while it ran; the old one would keep serving, with the new one's agents and command), or one that runs
/// without a backend (its files were replaced under it; macOS kills the helper processes, not the app). Either holds
/// the single-instance lock, so a launch would be handed to it and nothing would start. Returns whether one was
/// stopped.
pub fn stop_stale_desktop(home: &Path, options: &StaleOptions) -> bool {
    let Some(app) = std::fs::read_to_string(logs_directory(home).join("app.json"))
        .ok()
        .and_then(|contents| serde_json::from_str::<AppRecord>(&contents).ok())
    else {
        return false;
    };
    let Some(pid) = app.pid.filter(|pid| process::is_alive(*pid)) else {
        return false;
    };
    let reason = match (&options.version, &app.version) {
        (Some(installed), Some(running)) if installed != running => {
            format!("it is version {running}, and {installed} is installed")
        }
        _ => {
            let started = app
                .started
                .as_deref()
                .and_then(|started| chrono::DateTime::parse_from_rfc3339(started).ok())
                .map(SystemTime::from)
                .unwrap_or(SystemTime::UNIX_EPOCH);
            // Still starting (its backend registers within seconds): leave it be.
            if options.now.duration_since(started).unwrap_or_default() < options.grace {
                return false;
            }
            if server_record(home)
                .and_then(|server| server.pid)
                .is_some_and(process::is_alive)
            {
                return false;
            }
            "it runs without a backend, so nothing could start".to_string()
        }
    };
    log_in(
        home,
        &format!("stopping Co-Review (process {pid}): {reason}"),
    );
    if !process::terminate(pid) {
        return false;
    }
    // Ten seconds for it to go; then without ceremony, as a start waits on this.
    let deadline = Instant::now() + Duration::from_secs(10);
    while process::is_alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    if process::is_alive(pid) {
        process::kill(pid);
    }
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_command_finds_its_app_and_version() {
        let directory = tempfile::tempdir().unwrap();
        // macOS: Co-Review.app/Contents/{MacOS/Co-Review, Resources/app}
        let contents = directory.path().join("Co-Review.app/Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        std::fs::create_dir_all(contents.join("Resources/app/bin")).unwrap();
        std::fs::write(contents.join("MacOS/Co-Review"), "").unwrap();
        std::fs::write(contents.join("Resources/app/version"), "1.2.3\n").unwrap();
        let resources = contents.join("Resources/app");
        assert_eq!(
            bundle_executable(&resources),
            Some(contents.join("MacOS/Co-Review"))
        );
        let app = App::new(contents.join("MacOS/Co-Review"));
        assert_eq!(app.version.as_deref(), Some("1.2.3"));
        // Linux: Co-Review/{co-review-app, resources/app}
        let linux = directory.path().join("Co-Review");
        std::fs::create_dir_all(linux.join("resources/app/bin")).unwrap();
        std::fs::write(linux.join("co-review-app"), "").unwrap();
        std::fs::write(linux.join("resources/app/version"), "1.2.3").unwrap();
        assert_eq!(
            bundle_executable(&linux.join("resources/app")),
            Some(linux.join("co-review-app"))
        );
        assert_eq!(
            App::new(linux.join("co-review-app")).version.as_deref(),
            Some("1.2.3")
        );
    }

    #[test]
    fn the_app_is_found_beside_the_command_only_in_a_cargo_profile_directory() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("lapce"), "").unwrap();
        assert_eq!(built_app(directory.path()), None);
        std::fs::create_dir(directory.path().join(".fingerprint")).unwrap();
        assert_eq!(
            built_app(directory.path()),
            Some(directory.path().join("lapce"))
        );
    }
}
