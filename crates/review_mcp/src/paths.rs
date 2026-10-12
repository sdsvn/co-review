//! Paths as the TypeScript's `FileUri` and Node's `path` see them, so paths in results match.

use std::path::{MAIN_SEPARATOR, Path};

pub use review_bundle::payloads::relative_path;
pub use review_store::path_to_file_uri;

/// The filesystem path of a `file:` URI; other strings are returned as they are.
pub fn uri_to_path(uri: &str) -> String {
    review_store::file_uri_to_path(uri).unwrap_or_else(|| uri.to_string())
}

/// Node's `path.relative(from, to)` with `/` separators.
pub fn relative(from: &str, to: &str) -> String {
    relative_path(Path::new(from), Path::new(to))
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

/// `%XX` escapes decoded (invalid ones kept as they are), for query parameters.
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        let escaped = (byte == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(value) => {
                decoded.push(value);
                index += 3;
            }
            None => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
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

    #[test]
    fn query_values_decode() {
        assert_eq!(percent_decode("%2Ftmp%2Fa%20b"), "/tmp/a b");
        assert_eq!(percent_decode("100%"), "100%");
    }
}
