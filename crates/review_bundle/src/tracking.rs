//! Tracked targets of comments in a review directory's files: a new comment's target is tracked
//! from the file as it is (with a snapshot of that version), and when Co-Review edits a file itself
//! (an accepted suggestion) every comment on it follows that edit exactly.
//! Port of `anchor-snapshots.ts` and the resolution half of `anchor-tracker.ts`.
//!
//! Offsets are UTF-16 code units, as the TypeScript app stores them.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use review_anchor::{AnchorStatus, Edit, TrackedRange};
use review_model::{
    CodeLocation, DocAnchorType, LocationKind, Position, Range, ThreadStatus, content_version,
    is_overview_page,
};
use review_store::ReviewStore;

use crate::payloads::file_uri_to_path;

/// Where the snapshots of a workspace's commented files are: content-addressed by version.
pub fn snapshots_dir(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("snapshots")
}

pub fn save_snapshot(dir: &Path, version: &str, text: &str) -> Result<()> {
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
pub(crate) fn index_of(haystack: &[u16], needle: &[u16], from: usize) -> Option<usize> {
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
pub(crate) fn slice(units: &[u16], from: usize, to: usize) -> String {
    let to = to.min(units.len());
    let from = from.min(to);
    String::from_utf16_lossy(&units[from..to])
}

/// An edit Co-Review applied itself to the file at `uri`, whose text was `before`.
pub struct AppliedEdit<'a> {
    pub uri: &'a str,
    pub before: &'a str,
    pub edits: &'a [Edit],
}

/// Brings a review's tracked anchors up to date with its files: through `applied` exactly when the
/// anchor is on the version that edit was made against, else through a diff from the snapshot of
/// the anchor's version. Cheap when nothing changed (one hash per commented file).
pub fn resolve(store: &ReviewStore, review_id: &str, applied: Option<&AppliedEdit>) -> Result<()> {
    let Some(review) = store.get(review_id) else {
        return Ok(());
    };
    let dir = snapshots_dir(&store.workspace_dir(&review.workspace_root));
    let mut files: HashMap<String, Option<String>> = HashMap::new();
    let mut diffs: HashMap<String, Vec<Edit>> = HashMap::new();
    let mut updates: HashMap<String, CodeLocation> = HashMap::new();
    for thread in &review.threads {
        let location = &thread.location;
        let (Some(tracked), Some(uri)) = (&location.tracked, &location.uri) else {
            continue;
        };
        if thread.status == ThreadStatus::Resolved {
            continue;
        }
        let now = files
            .entry(uri.clone())
            .or_insert_with(|| file_uri_to_path(uri).and_then(|file| fs::read_to_string(file).ok()))
            .clone();
        let Some(now) = now else {
            // The file itself is gone.
            if tracked.status != AnchorStatus::Removed {
                let removed = TrackedRange {
                    status: AnchorStatus::Removed,
                    candidates: None,
                    ..tracked.clone()
                };
                updates.insert(
                    thread.id.clone(),
                    CodeLocation {
                        tracked: Some(removed),
                        ..location.clone()
                    },
                );
            }
            continue;
        };
        let version = content_version(&now);
        if version == tracked.version {
            continue;
        }
        let exact = applied.filter(|applied| {
            applied.uri == uri.as_str() && content_version(applied.before) == tracked.version
        });
        let before = match exact {
            Some(applied) => Some(applied.before.to_string()),
            None => fs::read_to_string(dir.join(&tracked.version)).ok(),
        };
        let edits = match (exact, &before) {
            (Some(applied), _) => Some(applied.edits.to_vec()),
            (None, Some(before)) => Some(
                diffs
                    .entry(format!("{uri}\n{}\n{version}", tracked.version))
                    .or_insert_with(|| review_anchor::edits_between(before, &now))
                    .clone(),
            ),
            (None, None) => None,
        };
        let next =
            review_anchor::track(tracked, before.as_deref(), &now, &version, edits.as_deref());
        save_snapshot(&dir, &version, &now)?;
        updates.insert(thread.id.clone(), follow(location, next, &now));
    }
    if !updates.is_empty() {
        store.retrack(review_id, &updates)?;
        collect(store, &review.workspace_root)?;
    }
    Ok(())
}

/// The location with its target where tracking found it: range and quote for code, quote and
/// occurrence for pages.
fn follow(location: &CodeLocation, tracked: TrackedRange, text: &str) -> CodeLocation {
    let placed = matches!(
        tracked.status,
        AnchorStatus::Active | AnchorStatus::Modified
    );
    if !placed {
        return CodeLocation {
            tracked: Some(tracked),
            ..location.clone()
        };
    }
    if location.kind == LocationKind::Document
        && let Some(doc_anchor) = &location.doc_anchor
    {
        let units: Vec<u16> = text.encode_utf16().collect();
        let quote: Vec<u16> = tracked.text.encode_utf16().collect();
        let mut occurrence = 0;
        let mut at = index_of(&units, &quote, 0);
        while let Some(found) = at.filter(|found| (*found as u64) < tracked.from) {
            occurrence += 1;
            at = index_of(&units, &quote, found + 1);
        }
        let mut doc_anchor = doc_anchor.clone();
        doc_anchor.exact = Some(tracked.text.clone());
        doc_anchor.occurrence = Some(occurrence);
        return CodeLocation {
            tracked: Some(tracked),
            doc_anchor: Some(doc_anchor),
            ..location.clone()
        };
    }
    let (start_line, start_character) = review_anchor::position_at(text, tracked.from as usize);
    let (end_line, end_character) = review_anchor::position_at(text, tracked.to as usize);
    let lines: Vec<&str> = text.split('\n').collect();
    let kind = if location.kind == LocationKind::Symbol {
        LocationKind::Symbol
    } else if start_line == end_line {
        LocationKind::Line
    } else {
        LocationKind::Range
    };
    let anchor = location
        .anchor
        .as_ref()
        .map(|anchor| review_model::CodeAnchor {
            text: tracked.text.clone(),
            before: start_line
                .checked_sub(1)
                .and_then(|line| lines.get(line))
                .map(|line| line.to_string()),
            after: lines.get(end_line + 1).map(|line| line.to_string()),
            // The token anchor was the old text's; the tracked range is what locates it now.
            syntax: if tracked.status == AnchorStatus::Active {
                anchor.syntax.clone()
            } else {
                None
            },
        });
    let position = |line: usize, character: usize| Position {
        line: u32::try_from(line).unwrap_or(u32::MAX),
        character: u32::try_from(character).unwrap_or(u32::MAX),
    };
    CodeLocation {
        kind,
        range: Some(Range {
            start: position(start_line, start_character),
            end: position(end_line, end_character),
        }),
        anchor,
        tracked: Some(tracked),
        ..location.clone()
    }
}

/// Deletes the workspace's snapshots that no comment of any of its reviews points to.
fn collect(store: &ReviewStore, workspace_root: &str) -> Result<()> {
    let dir = snapshots_dir(&store.workspace_dir(workspace_root));
    let used: HashSet<String> = store
        .list(workspace_root)
        .iter()
        .flat_map(|review| &review.threads)
        .filter_map(|thread| {
            thread
                .location
                .tracked
                .as_ref()
                .map(|tracked| tracked.version.clone())
        })
        .collect();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        if !used.contains(&*entry.file_name().to_string_lossy()) {
            fs::remove_file(entry.path()).ok();
        }
    }
    Ok(())
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
