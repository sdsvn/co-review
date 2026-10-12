//! File URI formation and path resolution, matching Theia's `FileUri` behaviour.
//!
//! The workspace hash is `sha256(file_uri_string)[0:16]` so this module must produce
//! the same URI string as the TypeScript app for every platform.

use std::path::{Path, PathBuf};

/// Characters that vscode-uri percent-encodes in the path component of a `file:` URI.
/// Matching the TS behaviour exactly: encode everything that is not unreserved or `/`,
/// specifically including `:`, `?`, `#`, `@`, `!`, `$`, `&`, `'`, `(`, `)`, `*`, `+`,
/// `,`, `;`, `=`, and space.
fn should_encode(byte: u8) -> bool {
    // RFC 3986 unreserved = ALPHA / DIGIT / "-" / "." / "_" / "~"
    // Plus `/` which is a path separator and is never encoded.
    !matches!(byte,
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' |
        b'-' | b'.' | b'_' | b'~' | b'/'
    )
}

fn percent_encode_path(path: &str) -> String {
    let mut result = String::with_capacity(path.len());
    for byte in path.as_bytes() {
        if should_encode(*byte) {
            result.push('%');
            result.push_str(&format!("{:02X}", byte));
        } else {
            result.push(*byte as char);
        }
    }
    result
}

/// Convert a filesystem path to a `file:` URI string, matching vscode-uri's `URI.file()`.
///
/// On Unix: `/home/user/project` → `file:///home/user/project`
/// On Windows: `C:\src\app` → `file:///c%3A/src/app`
///
/// Special characters (spaces, `#`, `?`, non-ASCII, etc.) are percent-encoded.
pub fn path_to_file_uri(path: &str) -> String {
    let normalized = if cfg!(target_os = "windows") {
        // On Windows, convert backslashes to forward slashes and lowercase the drive letter
        let mut p = path.replace('\\', "/");
        if p.len() >= 2 && p.as_bytes()[1] == b':' {
            let drive = (p.as_bytes()[0] as char).to_ascii_lowercase();
            p.replace_range(0..1, &drive.to_string());
        }
        if !p.starts_with('/') {
            format!("/{p}")
        } else {
            p
        }
    } else {
        // On Unix, the path as-is (should already start with /)
        path.to_string()
    };

    let encoded = percent_encode_path(&normalized);
    format!("file://{encoded}")
}

/// Extract the filesystem path from a `file:` URI, matching vscode-uri's `URI.fsPath`.
pub fn file_uri_to_path(uri: &str) -> Option<String> {
    let path_part = uri.strip_prefix("file://")?;

    // Percent-decode the path
    let decoded = percent_decode(path_part);

    if cfg!(target_os = "windows") {
        // On Windows, strip leading / before drive letter: /c:/... → c:/...
        let trimmed = decoded.strip_prefix('/').unwrap_or(&decoded);
        Some(trimmed.replace('/', "\\"))
    } else {
        Some(decoded)
    }
}

/// `%XX` escapes decoded (invalid ones kept as they are), as UTF-8.
pub fn percent_decode(input: &str) -> String {
    let src = input.as_bytes();
    let mut decoded = Vec::with_capacity(src.len());
    let mut i = 0;
    while i < src.len() {
        if src[i] == b'%'
            && i + 2 < src.len()
            && let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&src[i + 1..i + 3]).unwrap_or(""),
                16,
            )
        {
            decoded.push(byte);
            i += 3;
        } else {
            decoded.push(src[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Resolve a path, following symlinks where they exist (matching the TS `realPath`).
///
/// On macOS, `/var` is `/private/var`, where `$TMPDIR` lives. This function follows
/// symlinks for the parts of the path that exist, keeping the rest as-is.
pub fn real_path(path: &Path) -> PathBuf {
    // Try the whole path first
    // On Windows, std's canonicalize returns verbatim paths (\\?\C:\...) that git and file URIs reject; dunce doesn't.
    if let Ok(real) = dunce::canonicalize(path) {
        return real;
    }
    // The path doesn't fully exist; resolve the longest existing prefix
    let Some(parent) = path.parent().filter(|p| *p != path) else {
        return path.to_path_buf();
    };
    let Some(file_name) = path.file_name() else {
        return path.to_path_buf();
    };
    real_path(parent).join(file_name)
}

/// A `file:` URI with its path resolved via `real_path`; other URIs returned as-is.
pub fn real_uri(uri: &str) -> String {
    if !uri.starts_with("file:") {
        return uri.to_string();
    }
    let Some(path_str) = file_uri_to_path(uri) else {
        return uri.to_string();
    };
    let resolved = real_path(Path::new(&path_str));
    path_to_file_uri(&resolved.to_string_lossy())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_path_to_file_uri_unix() {
        assert_eq!(path_to_file_uri("/tmp/test"), "file:///tmp/test");
        assert_eq!(
            path_to_file_uri("/tmp/my project"),
            "file:///tmp/my%20project"
        );
        assert_eq!(path_to_file_uri("/tmp/#hash"), "file:///tmp/%23hash");
        assert_eq!(path_to_file_uri("/tmp/a?b"), "file:///tmp/a%3Fb");
        assert_eq!(
            path_to_file_uri("/tmp/100%done"),
            "file:///tmp/100%25done"
        );
    }

    #[test]
    fn test_path_to_file_uri_cafe() {
        // "café" in UTF-8 is 63 61 66 C3 A9
        assert_eq!(
            path_to_file_uri("/tmp/café"),
            "file:///tmp/caf%C3%A9"
        );
    }

    #[test]
    fn test_file_uri_roundtrip() {
        let path = "/tmp/test";
        let uri = path_to_file_uri(path);
        assert_eq!(file_uri_to_path(&uri).unwrap(), path);
    }

    #[test]
    fn test_file_uri_roundtrip_non_ascii() {
        // Ensures percent_decode correctly reassembles multi-byte UTF-8
        let path = "/tmp/café";
        let uri = path_to_file_uri(path);
        assert_eq!(uri, "file:///tmp/caf%C3%A9");
        assert_eq!(file_uri_to_path(&uri).unwrap(), path);
    }

    #[test]
    fn test_real_path_existing() {
        // /tmp on macOS is a symlink to /private/tmp
        let resolved = real_path(Path::new("/tmp"));
        #[cfg(target_os = "macos")]
        assert_eq!(resolved, PathBuf::from("/private/tmp"));
    }

    #[test]
    fn test_real_path_nonexistent() {
        let resolved = real_path(Path::new("/tmp/nonexistent_dir_12345/subdir"));
        #[cfg(target_os = "macos")]
        assert!(resolved.starts_with("/private/tmp"));
        assert!(resolved.ends_with("nonexistent_dir_12345/subdir"));
    }

    #[test]
    fn test_real_uri_passthrough() {
        assert_eq!(real_uri("https://example.com"), "https://example.com");
    }

    /// Verify our hash matches the TS hash for known URIs.
    #[test]
    fn test_workspace_hash_matches_ts() {
        use sha2::{Digest, Sha256};
        let cases = [
            ("file:///tmp/test", "e37e1da7cf9e40ef"),
            (
                "file:///Users/sarathsadasivanpillai/projects/co-review",
                "60e5ecfd561229fb",
            ),
            ("file:///c%3A/src/app", "f9d95e0a7cb23169"),
            ("file:///home/user/project", "80b9a6ddd210bfa9"),
        ];
        for (uri, expected_hash) in cases {
            let hash = format!("{:x}", Sha256::digest(uri.as_bytes()));
            let prefix = &hash[..16];
            assert_eq!(prefix, expected_hash, "hash mismatch for {uri}");
        }
    }
}
