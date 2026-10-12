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

/// The desktop app's `resources/app` when this command is the one bundled in it (`resources/app/bin/co-review`).
pub fn bundled_resources() -> Option<PathBuf> {
    let directory = executable_directory()?;
    if directory.file_name()? != "bin" {
        return None;
    }
    let resources = directory.parent()?;
    (resources.join("package.json").is_file() && resources.join("integrations").is_dir())
        .then(|| resources.to_path_buf())
}

/// The repository checkout this command was built in (`target/<profile>/co-review`), if any.
pub fn checkout_root() -> Option<PathBuf> {
    let directory = executable_directory()?;
    directory
        .ancestors()
        .take(5)
        .find(|candidate| {
            candidate
                .join("applications/electron/package.json")
                .is_file()
        })
        .map(Path::to_path_buf)
}

/// Where the files that ship with Co-Review are (`integrations/`, `plugin/`, `bin/`): the app this command is
/// bundled in, else the checkout it was built in, else the installed desktop app.
pub fn distribution_root() -> Option<PathBuf> {
    bundled_resources().or_else(checkout_root).or_else(|| {
        crate::app::installed_desktop_app().and_then(|executable| resources_of(&executable))
    })
}

/// The desktop app's `resources/app` for its executable (macOS bundle or Linux/Windows directory layout).
pub fn resources_of(executable: &Path) -> Option<PathBuf> {
    let directory = executable.parent()?;
    let candidates = [
        directory.join("../Resources/app"),
        directory.join("resources/app"),
    ];
    candidates
        .into_iter()
        .map(|candidate| normalize(&candidate))
        .find(|candidate| candidate.join("package.json").is_file())
}

/// The `version` field of a package.json.
pub fn package_version(package_json: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(package_json).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&contents).ok()?;
    parsed
        .get("version")?
        .as_str()
        .filter(|version| !version.is_empty())
        .map(str::to_string)
}

/// This Co-Review's version: the desktop app's it is bundled in, or the checkout's app.
pub fn version() -> String {
    bundled_resources()
        .and_then(|resources| package_version(&resources.join("package.json")))
        .or_else(|| {
            checkout_root()
                .and_then(|root| package_version(&root.join("applications/electron/package.json")))
        })
        .unwrap_or_else(|| "unknown".to_string())
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
