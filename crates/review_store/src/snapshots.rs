//! Snapshots of commented files and the tracked target of a new comment, which every new thread
//! gets (so no caller can create one untracked). Port of `anchor-snapshots.ts`.
//!
//! Offsets are UTF-16 code units, as the TypeScript app stores them.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use review_anchor::AnchorStatus;
use review_model::{CodeLocation, DocAnchorType, LocationKind, content_version, is_overview_page};

use crate::file_uri::file_uri_to_path;

/// Where the snapshots of a workspace's commented files are: content-addressed by version.
pub fn snapshots_dir(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("snapshots")
}

pub fn save_snapshot(dir: &Path, version: &str, text: &str) -> std::io::Result<()> {
    let file = dir.join(version);
    if !file.exists() {
        fs::create_dir_all(dir)?;
        fs::write(file, text)?;
    }
    Ok(())
}

/// The tracked target of a new comment, from the file as it is now: a line or range comment's
/// lines, or a document's quoted text found in its Markdown source (only when that place is
/// clear). Anything else, or a file that doesn't read as the comment says, is left as it is.
pub fn track_new(workspace_dir: &Path, location: CodeLocation) -> CodeLocation {
    let Some(uri) = location.uri.as_deref() else {
        return location;
    };
    if location.tracked.is_some() || !uri.starts_with("file:") || is_overview_page(uri) {
        return location;
    }
    let Some(text) = file_uri_to_path(uri).and_then(|file| fs::read_to_string(file).ok()) else {
        return location;
    };
    let version = content_version(&text);
    let units: Vec<u16> = text.encode_utf16().collect();
    let range = match (&location.kind, &location.range, &location.doc_anchor) {
        (LocationKind::Line | LocationKind::Range | LocationKind::Symbol, Some(range), _) => {
            let from = review_anchor::offset_at(
                &text,
                range.start.line as usize,
                range.start.character as usize,
            );
            let to = review_anchor::offset_at(
                &text,
                range.end.line as usize,
                range.end.character as usize,
            );
            let quote = location.anchor.as_ref().map(|anchor| anchor.text.as_str());
            // The editor's text is the file's (an unsaved buffer isn't tracked from the disk).
            (to > from
                && quote.is_none_or(|quote| quote.is_empty() || slice(&units, from, to) == quote))
            .then_some((from, to))
        }
        (LocationKind::Document, _, Some(doc_anchor))
            if doc_anchor.anchor_type == DocAnchorType::Text =>
        {
            doc_anchor
                .exact
                .as_deref()
                .filter(|exact| !exact.is_empty())
                .and_then(|exact| {
                    source_range(
                        &units,
                        exact,
                        doc_anchor.prefix.as_deref().unwrap_or(""),
                        doc_anchor.suffix.as_deref().unwrap_or(""),
                    )
                })
        }
        _ => None,
    };
    let Some((from, to)) = range else {
        return location;
    };
    if let Err(error) = save_snapshot(&snapshots_dir(workspace_dir), &version, &text) {
        log::error!("[co-review] saving a snapshot failed: {error:#}");
    }
    CodeLocation {
        tracked: Some(review_anchor::range_in(
            &text,
            &version,
            from,
            to,
            AnchorStatus::Active,
            None,
        )),
        ..location
    }
}

/// Letters and digits only, for comparing rendered text (a document's) with its Markdown source.
fn plain(text: &str) -> String {
    static NOT_WORD: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}]+").ok());
    match NOT_WORD.as_ref() {
        Some(regex) => regex.replace_all(text, " ").trim().to_string(),
        None => text.trim().to_string(),
    }
}

/// Where rendered `exact` is in a Markdown `source`: its one occurrence, or the one whose context
/// reads best.
fn source_range(source: &[u16], exact: &str, prefix: &str, suffix: &str) -> Option<(usize, usize)> {
    let exact: Vec<u16> = exact.encode_utf16().collect();
    let prefix = plain(prefix);
    let suffix = plain(suffix);
    let mut places: Vec<(usize, u8)> = Vec::new();
    let mut search_from = 0;
    while let Some(at) = index_of(source, &exact, search_from) {
        let before = plain(&slice(source, at.saturating_sub(200), at));
        let end = at + exact.len();
        let after = plain(&slice(source, end, end + 200));
        let score = u8::from(!prefix.is_empty() && before.ends_with(&prefix))
            + u8::from(!suffix.is_empty() && after.starts_with(&suffix));
        places.push((at, score));
        search_from = at + 1;
    }
    // Stable, as JavaScript's sort: equal scores keep document order.
    places.sort_by_key(|place| std::cmp::Reverse(place.1));
    match places.as_slice() {
        [] => None,
        [best, second, ..] if second.1 == best.1 => None,
        [best, ..] => Some((best.0, best.0 + exact.len())),
    }
}

/// JavaScript's `indexOf` over UTF-16 code units.
pub fn index_of(haystack: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if from > haystack.len() {
        return needle.is_empty().then_some(haystack.len());
    }
    if needle.is_empty() {
        return Some(from);
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|position| position + from)
}

/// JavaScript's `slice` over UTF-16 code units.
pub fn slice(units: &[u16], from: usize, to: usize) -> String {
    let to = to.min(units.len());
    let from = from.min(to);
    String::from_utf16_lossy(&units[from..to])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn source_range_picks_the_occurrence_whose_context_reads_best() {
        let source = units("The size is set. The size defaults to 100.");
        assert_eq!(
            source_range(&source, "The size", "is set.", "defaults"),
            Some((17, 25))
        );
        // Two equally good places: not clear, so not tracked.
        assert_eq!(source_range(&source, "The size", "", ""), None);
        assert_eq!(source_range(&source, "absent", "", ""), None);
    }

    #[test]
    fn index_of_matches_javascript() {
        let text = units("aé😀b");
        assert_eq!(index_of(&text, &units("b"), 0), Some(4));
        assert_eq!(index_of(&text, &units(""), 2), Some(2));
        assert_eq!(index_of(&text, &units(""), 9), Some(5));
        assert_eq!(slice(&text, 1, 4), "é😀");
    }
}
