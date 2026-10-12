//! Tracked targets of comments in a review directory's files: when Co-Review edits a file itself
//! (an accepted suggestion) every comment on it follows that edit exactly. A new comment's target
//! is tracked by `ReviewStore::create_thread` (see `review_store::snapshots`).
//! Port of the resolution half of `anchor-tracker.ts`.
//!
//! Offsets are UTF-16 code units, as the TypeScript app stores them.

use std::collections::{HashMap, HashSet};
use std::fs;

use anyhow::Result;
use review_anchor::{AnchorStatus, Edit, TrackedRange, index_of};
use review_model::{CodeLocation, LocationKind, Position, Range, ThreadStatus, content_version};
use review_store::snapshots::{save_snapshot, snapshots_dir};
use review_store::{ReviewStore, file_uri_to_path};

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
