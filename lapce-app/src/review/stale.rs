//! A stale app quits (cli spec, "Stale versions"): once an update replaced the app on disk (its
//! `resources/app/version` is not the running version), the running app would keep serving agents with the old
//! version and hold the single-instance lock, so launches never reach the new one. Checked every minute in a
//! background start and when the app is activated, off the UI thread. It quits only with no window open: a reviewer
//! mid-review keeps their windows, and the app quits once the last one closes. Quitting goes through
//! `ReviewCore::shut_down`, so the agents of open windows are told; a `SIGTERM` (how `co-review` stops a stale app)
//! takes the same path.
//!
//! The Theia app's `watchBackend` / `quitStale` (`electron-main/review-electron-main-module.ts`).

use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

/// How often a background start looks at the version on disk.
const CHECK_EVERY: Duration = Duration::from_secs(60);

/// The running app's version, the one packaging writes to `resources/app/version`.
const OWN_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The `resources/app/version` of the app whose executable is `executable` (`packaging/package.sh`):
/// `<Name>.app/Contents/MacOS/<Name>` → `Contents/Resources/app/version` on macOS, `<dir>/resources/app/version`
/// elsewhere. None for a build that isn't packaged (`target/debug/lapce`).
fn version_file(executable: &Path) -> Option<PathBuf> {
    let directory = executable.parent()?;
    let candidates = [
        directory.parent()?.join("Resources/app/version"),
        directory.join("resources/app/version"),
    ];
    candidates.into_iter().find(|candidate| candidate.is_file())
}

/// The version installed on disk when it isn't `own` (an update replaced the app); None when it is, or the app isn't
/// packaged, or the file is being replaced right now.
fn replaced_by(executable: &Path, own: &str) -> Option<String> {
    let text = std::fs::read_to_string(version_file(executable)?).ok()?;
    let on_disk = text.trim();
    (!on_disk.is_empty() && on_disk != own).then(|| on_disk.to_string())
}

/// This app's replacement on disk, if any. Reads a file: call it off the UI thread.
pub(super) fn check() -> Option<String> {
    let executable = std::env::current_exe().ok()?;
    let executable = std::fs::canonicalize(&executable).unwrap_or(executable);
    replaced_by(&executable, OWN_VERSION)
}

/// Every minute, on a background thread: reports the version on disk to `stale` once it is another one.
pub(super) fn watch(stale: mpsc::Sender<String>) {
    let watching = std::thread::Builder::new()
        .name("ReviewStaleVersion".to_owned())
        .spawn(move || {
            loop {
                std::thread::sleep(CHECK_EVERY);
                if let Some(on_disk) = check() {
                    stale.send(on_disk).ok();
                    return;
                }
            }
        });
    if let Err(error) = watching {
        tracing::error!("the app will not notice an update replacing it: {error}");
    }
}

/// `SIGTERM` quits the app as its own Quit does (agents told, `server.json` removed) instead of ending the process
/// on the spot: `co-review` stops a stale app with it.
#[cfg(unix)]
pub(super) fn quit_on_terminate() {
    let mut signals = match signal_hook::iterator::Signals::new([signal_hook::consts::SIGTERM]) {
        Ok(signals) => signals,
        Err(error) => {
            tracing::error!("SIGTERM will end the app without telling agents: {error}");
            return;
        }
    };
    let listening = std::thread::Builder::new()
        .name("ReviewTerminate".to_owned())
        .spawn(move || {
            if signals.forever().next().is_some() {
                tracing::info!("asked to quit (SIGTERM): quitting");
                floem::quit_app();
            }
        });
    if let Err(error) = listening {
        tracing::error!("SIGTERM will end the app without telling agents: {error}");
    }
}

#[cfg(not(unix))]
pub(super) fn quit_on_terminate() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_packaged_app_is_stale_once_another_version_is_on_disk() {
        let scratch = tempfile::tempdir().unwrap();
        // macOS: Co-Review.app/Contents/{MacOS/Co-Review, Resources/app/version}
        let contents = scratch.path().join("Co-Review.app/Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        std::fs::create_dir_all(contents.join("Resources/app")).unwrap();
        let executable = contents.join("MacOS/Co-Review");
        std::fs::write(&executable, "").unwrap();
        std::fs::write(contents.join("Resources/app/version"), "0.6.3\n").unwrap();
        assert_eq!(replaced_by(&executable, "0.6.3"), None);
        std::fs::write(contents.join("Resources/app/version"), "0.6.4\n").unwrap();
        assert_eq!(replaced_by(&executable, "0.6.3").as_deref(), Some("0.6.4"));
        // Being replaced right now: nothing to tell yet.
        std::fs::write(contents.join("Resources/app/version"), "").unwrap();
        assert_eq!(replaced_by(&executable, "0.6.3"), None);

        // Linux and Windows: <dir>/{co-review-app, resources/app/version}
        let linux = scratch.path().join("Co-Review");
        std::fs::create_dir_all(linux.join("resources/app")).unwrap();
        std::fs::write(linux.join("resources/app/version"), "0.7.0").unwrap();
        assert_eq!(
            replaced_by(&linux.join("co-review-app"), "0.6.3").as_deref(),
            Some("0.7.0")
        );

        // Not packaged (target/debug/lapce): never stale.
        let built = scratch.path().join("target/debug");
        std::fs::create_dir_all(&built).unwrap();
        assert_eq!(replaced_by(&built.join("lapce"), "0.6.3"), None);
    }
}
