//! Paths as the TypeScript's `FileUri` and Node's `path` see them, so paths in results match.

use std::path::{MAIN_SEPARATOR, Path};

pub use review_store::path_to_file_uri;

/// The filesystem path of a `file:` URI; other strings are returned as they are.
pub fn uri_to_path(uri: &str) -> String {
    review_store::file_uri_to_path(uri).unwrap_or_else(|| uri.to_string())
}

/// Node's `path.relative(from, to)` with `/` separators.
pub fn relative(from: &str, to: &str) -> String {
    review_bundle::payloads::relative_path(Path::new(from), Path::new(to))
}

/// Node's `path.resolve(base, relative)`, `.` and `..` folded away.
pub fn resolve(base: &str, relative: &str) -> String {
    review_bundle::payloads::resolve_path(&Path::new(base).join(relative))
        .to_string_lossy()
        .into_owned()
}

/// Whether `file` is `root` itself or inside it (both resolved).
pub fn is_within(root: &str, file: &str) -> bool {
    file == root || file.starts_with(&format!("{root}{MAIN_SEPARATOR}"))
}

/// The last segment of a path.
pub fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A path resolved through symlinks (`/var` is `/private/var` on macOS), as the TypeScript's `realPath`.
pub fn real_path(path: &str) -> String {
    review_store::real_path(Path::new(path))
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_folds_dots() {
        assert_eq!(resolve("/repo", "../outside.ts"), "/outside.ts");
        assert_eq!(resolve("/repo", "."), "/repo");
        assert_eq!(resolve("/repo", "./src/"), "/repo/src");
        assert!(is_within("/repo", "/repo"));
        assert!(is_within("/repo", "/repo/src"));
        assert!(!is_within("/repo", "/repository"));
    }
}
