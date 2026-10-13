//! The `co-review` command stops a desktop app that must not be used before starting one
//! (port of extensions/review/test/stale-desktop.test.mjs).
#![cfg(unix)]

use std::fs;
use std::process::Command;
use std::time::{Duration, SystemTime};

use review_cli::app::{StaleOptions, stop_stale_desktop};
use review_cli::process;
use serde_json::json;

/// A process that is nobody's child here (like a real app): a stopped one is gone, not a zombie waiting to be reaped.
fn sleeper() -> u32 {
    let output = Command::new("sh")
        .args(["-c", "sleep 300 >/dev/null 2>&1 & echo $!"])
        .output()
        .expect("sleeper");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("pid")
}

fn gone(pid: u32) -> bool {
    for _ in 0..50 {
        if !process::is_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    !process::is_alive(pid)
}

/// A home with an "app" (a sleeping process) started `started`, of `version`, and optionally a live "backend".
struct Home {
    directory: tempfile::TempDir,
    app: u32,
    backend: Option<u32>,
}

impl Home {
    fn new(version: &str, started: &str, backend: bool) -> Self {
        let directory = tempfile::tempdir().expect("temporary directory");
        fs::create_dir(directory.path().join("logs")).expect("logs");
        let app = sleeper();
        let backend = backend.then(sleeper);
        fs::write(
            directory.path().join("logs/app.json"),
            json!({ "pid": app, "version": version, "started": started }).to_string(),
        )
        .expect("app.json");
        if let Some(backend) = backend {
            fs::write(
                directory.path().join("server.json"),
                json!({ "url": "http://127.0.0.1:1", "pid": backend }).to_string(),
            )
            .expect("server.json");
        }
        Home {
            directory,
            app,
            backend,
        }
    }

    /// Stops the app if it must not be used; `version` stands in for the installed one.
    fn stop(&self, version: Option<&str>) -> bool {
        stop_stale_desktop(
            self.directory.path(),
            &StaleOptions {
                now: SystemTime::now(),
                grace: Duration::from_secs(90),
                version: version.map(str::to_string),
            },
        )
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        process::kill(self.app);
        if let Some(backend) = self.backend {
            process::kill(backend);
        }
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

const LONG_AGO: &str = "2026-01-01T00:00:00.000Z";

#[test]
fn an_app_of_the_installed_version_with_a_live_backend_is_left_alone() {
    let home = Home::new("0.6.1", LONG_AGO, true);
    assert!(!home.stop(Some("0.6.1")));
    assert!(process::is_alive(home.app));
}

#[test]
fn an_app_still_starting_without_a_backend_yet_is_left_alone() {
    let home = Home::new("0.6.1", &now(), false);
    assert!(!home.stop(Some("0.6.1")));
    assert!(process::is_alive(home.app));
}

#[test]
fn an_app_running_without_a_backend_is_stopped() {
    let home = Home::new("0.6.1", LONG_AGO, false);
    assert!(home.stop(Some("0.6.1")));
    assert!(gone(home.app));
    let log = fs::read_to_string(home.directory.path().join("logs/cli.log")).expect("cli.log");
    assert!(log.contains(&format!(
        "stopping Co-Review (process {}): it runs without a backend, so nothing could start",
        home.app
    )));
}

#[test]
fn an_app_of_another_version_is_stopped_even_with_its_backend_up() {
    let home = Home::new("0.6.0", &now(), true);
    assert!(home.stop(Some("0.6.1")));
    assert!(gone(home.app));
}

#[test]
fn in_a_checkout_without_a_version_of_its_own_only_the_backend_counts() {
    let home = Home::new("0.6.0", LONG_AGO, true);
    assert!(!home.stop(None));
    assert!(process::is_alive(home.app));
}

#[test]
fn no_app_record_or_a_dead_app_is_nothing_to_stop() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let options = StaleOptions::new(Some("0.6.1".to_string()));
    assert!(!stop_stale_desktop(directory.path(), &options));
    fs::create_dir(directory.path().join("logs")).expect("logs");
    fs::write(
        directory.path().join("logs/app.json"),
        json!({ "pid": 9_999_999, "version": "0.6.0" }).to_string(),
    )
    .expect("app.json");
    assert!(!stop_stale_desktop(directory.path(), &options));
}
