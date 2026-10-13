//! Where Co-Review keeps its data and where the files that ship with it are.

use std::path::{Component, Path, PathBuf};

/// The user's home directory.
pub fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// `$CO_REVIEW_HOME`, else `~/.co-review`.
pub fn co_review_home() -> PathBuf {
    match std::env::var_os("CO_REVIEW_HOME") {
        Some(home) if !home.is_empty() => PathBuf::from(home),
        _ => user_home().join(".co-review"),
    }
}

pub fn logs_directory(home: &Path) -> PathBuf {
    home.join("logs")
}

fn executable_directory() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let executable = std::fs::canonicalize(&executable).unwrap_or(executable);
    executable.parent().map(Path::to_path_buf)
}

/// The file in the app's `resources/app` that holds the app's version (written when it is packaged).
const VERSION_FILE: &str = "version";

/// The app's `resources/app` when this command is the one bundled in it (`resources/app/bin/co-review`; the layout
/// is in packaging/package.sh).
pub fn bundled_resources() -> Option<PathBuf> {
    let directory = executable_directory()?;
    if directory.file_name()? != "bin" {
        return None;
    }
    let resources = directory.parent()?;
    (resources.join(VERSION_FILE).is_file() && resources.join("integrations").is_dir())
        .then(|| resources.to_path_buf())
}

/// The repository checkout this command was built in (`target/<profile>/co-review`), if any.
pub fn checkout_root() -> Option<PathBuf> {
    let directory = executable_directory()?;
    directory
        .ancestors()
        .take(5)
        .find(|candidate| candidate.join("crates/review_cli/Cargo.toml").is_file())
        .map(Path::to_path_buf)
}

/// Where the files that ship with Co-Review are (`integrations/`, `plugin/`, `bin/`): the app this command is
/// bundled in, else the checkout it was built in, else the installed desktop app.
pub fn distribution_root() -> Option<PathBuf> {
    bundled_resources().or_else(checkout_root).or_else(|| {
        crate::app::installed_app().and_then(|executable| resources_of(&executable))
    })
}

/// The app's `resources/app` for its executable: `<Name>.app/Contents/MacOS/<Name>` → `Contents/Resources/app` on
/// macOS, `<dir>/<executable>` → `<dir>/resources/app` on Linux and Windows.
pub fn resources_of(executable: &Path) -> Option<PathBuf> {
    let directory = executable.parent()?;
    let candidates = [
        directory.join("../Resources/app"),
        directory.join("resources/app"),
    ];
    candidates
        .into_iter()
        .map(|candidate| normalize(&candidate))
        .find(|candidate| candidate.join(VERSION_FILE).is_file())
}

/// The version of the app whose `resources/app` is `resources`, as it was packaged.
pub fn installed_version(resources: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(resources.join(VERSION_FILE)).ok()?;
    let version = contents.trim();
    (!version.is_empty()).then(|| version.to_string())
}

/// This Co-Review's version: the app's, which this command is built with (Cargo.toml's workspace version).
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// `path` made absolute against the current directory, with `.` and `..` resolved lexically (as Node's
/// `path.resolve`, which does not follow symbolic links).
pub fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        normalize(path)
    } else {
        let current = std::env::current_dir().unwrap_or_default();
        normalize(&current.join(path))
    }
}

pub fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    result.push(component);
                }
            }
            other => result.push(other),
        }
    }
    result
}

/// `encodeURIComponent`.
pub fn encode_uri_component(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => encoded.push(byte as char),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

pub fn read_json(path: &Path) -> Option<serde_json::Value> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_like_node_path_resolve() {
        assert_eq!(
            normalize(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
    }

    #[test]
    fn encodes_like_encode_uri_component() {
        assert_eq!(
            encode_uri_component("/a b/c(1)!é"),
            "%2Fa%20b%2Fc(1)!%C3%A9"
        );
    }
}
