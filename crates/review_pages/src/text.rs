//! String helpers that behave like the JavaScript the pages were first written in, so the Markdown
//! stays byte-for-byte the same.

/// JavaScript's whitespace (`String.trim`, regex `\s`): Unicode White_Space without U+0085, plus U+FEFF.
pub(crate) fn is_js_space(character: char) -> bool {
    (character.is_whitespace() && character != '\u{85}') || character == '\u{FEFF}'
}

pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

pub(crate) fn js_trim_end(text: &str) -> &str {
    text.trim_end_matches(is_js_space)
}

/// The first `length` UTF-16 code units, as `String.slice(0, length)`.
pub(crate) fn truncate_utf16(text: &str, length: usize) -> String {
    if text.len() <= length {
        return text.to_string();
    }
    let units: Vec<u16> = text.encode_utf16().take(length).collect();
    String::from_utf16_lossy(&units)
}

pub(crate) fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// The filesystem path of a URI, as vscode-uri's `fsPath` (on POSIX).
pub(crate) fn file_uri_to_path(uri: &str) -> String {
    let after_scheme = uri.split_once(':').map_or(uri, |(_, rest)| rest);
    let path = match after_scheme.strip_prefix("//") {
        Some(authority_and_path) => authority_and_path
            .find('/')
            .map_or("", |index| &authority_and_path[index..]),
        None => after_scheme,
    };
    review_store::percent_decode(path)
}

fn normalized_segments(path: &str) -> Vec<&str> {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            segment => segments.push(segment),
        }
    }
    segments
}

/// `path.relative(from, to)` for absolute POSIX paths.
pub(crate) fn relative_path(from: &str, to: &str) -> String {
    let from = normalized_segments(from);
    let to = normalized_segments(to);
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts: Vec<&str> = vec![".."; from.len() - common];
    parts.extend(&to[common..]);
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_match_node() {
        assert_eq!(relative_path("/repo", "/repo/src/a.rs"), "src/a.rs");
        assert_eq!(relative_path("/repo/", "/repo"), "");
        assert_eq!(relative_path("/repo", "/other/a.rs"), "../other/a.rs");
    }

    #[test]
    fn file_uris_decode() {
        assert_eq!(file_uri_to_path("file:///a%20b/c.rs"), "/a b/c.rs");
    }

    #[test]
    fn truncation_counts_utf16_units() {
        assert_eq!(truncate_utf16("abc", 2), "ab");
        assert_eq!(truncate_utf16("é".repeat(3).as_str(), 2), "éé");
    }
}
