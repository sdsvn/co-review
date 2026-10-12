//! Comment anchor tracking: carries a comment's target through file edits and classifies
//! what happened to it. Port of `extensions/review/src/common/anchor-tracking.ts`.
//!
//! All offsets are UTF-16 code unit offsets for compatibility with the JavaScript implementation
//! and stored data.

mod change_set;
mod dmp;
mod utf16;

use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use review_model::{AnchorStatus, TrackedRange, TrackedRangeCandidate};

use change_set::ChangeSet;
use dmp::{cleanup_semantic, make_diff, match_bitap, x_index, DiffOp};
use utf16::{is_word_char, Utf16Text};

/// A replacement in the old text: characters `from`--`to` become `insert`.
/// All offsets are UTF-16 code unit offsets.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edit {
    pub from: usize,
    pub to: usize,
    pub insert: String,
}

impl Edit {
    /// The length of the inserted text in UTF-16 code units.
    pub fn insert_len(&self) -> usize {
        self.insert.encode_utf16().count()
    }
}

/// Tuning constants for anchor tracking.
pub struct Tracking {
    /// Characters of context kept on each side of the target.
    pub context: usize,
    /// Modified (not removed) when at least this share of the target's characters survive.
    pub modified: f64,
    /// Ambiguous when the second-best place scores within this of the best.
    pub ambiguous_margin: f64,
    /// Fuzzy match threshold (0 = exact, 1 = anything).
    pub fuzzy_threshold: f64,
    /// How far from the expected place to look for a fuzzy match.
    pub fuzzy_distance: usize,
}

/// The default tracking constants, matching the TypeScript implementation.
pub const TRACKING: Tracking = Tracking {
    context: 40,
    modified: 0.7,
    ambiguous_margin: 0.1,
    fuzzy_threshold: 0.3,
    fuzzy_distance: 1000,
};

/// A target in `text` from `from` to `to`: its text and context, in version `version`.
/// All offsets are UTF-16 code unit offsets.
pub fn range_in(
    text: &str,
    version: &str,
    from: usize,
    to: usize,
    status: AnchorStatus,
    original: Option<&str>,
) -> TrackedRange {
    let t = Utf16Text::new(text);
    let ctx = TRACKING.context;
    let prefix_start = from.saturating_sub(ctx);
    let suffix_end = (to + ctx).min(t.len_utf16());

    let text_slice = t.slice_str(from, to);
    let prefix = t.slice_str(prefix_start, from);
    let suffix = t.slice_str(to, suffix_end);
    let orig = original.unwrap_or(&text_slice).to_string();

    TrackedRange {
        version: version.to_string(),
        from: from as u64,
        to: to as u64,
        text: text_slice,
        prefix,
        suffix,
        status,
        original: orig,
        candidates: None,
        extra: serde_json::Map::new(),
    }
}

/// The edits that turn `before` into `after`: a line diff first, then characters within
/// changed lines, semantically cleaned up.
pub fn edits_between(before: &str, after: &str) -> Vec<Edit> {
    let before_u16: Vec<u16> = before.encode_utf16().collect();
    let after_u16: Vec<u16> = after.encode_utf16().collect();

    let mut diffs = make_diff(&before_u16, &after_u16, true, Duration::from_secs(2));
    cleanup_semantic(&mut diffs);

    let mut edits = Vec::new();
    let mut at: usize = 0;
    let mut i = 0;
    while i < diffs.len() {
        if diffs[i].is_equal() {
            at += diffs[i].text().len();
            i += 1;
            continue;
        }
        // Collect a run of non-equal diffs
        let mut removed: usize = 0;
        let mut inserted_units: Vec<u16> = Vec::new();
        while i < diffs.len() && !diffs[i].is_equal() {
            match &diffs[i] {
                DiffOp::Delete(t) => removed += t.len(),
                DiffOp::Insert(t) => inserted_units.extend_from_slice(t),
                _ => {}
            }
            i += 1;
        }
        let inserted = String::from_utf16_lossy(&inserted_units);
        edits.push(Edit {
            from: at,
            to: at + removed,
            insert: inserted,
        });
        at += removed;
    }
    edits
}

/// How alike two texts are, 0 to 1: the share of their characters in common.
/// Uses UTF-16 code unit lengths to match JavaScript.
pub fn similarity(a: &str, b: &str) -> f64 {
    let a_u16: Vec<u16> = a.encode_utf16().collect();
    let b_u16: Vec<u16> = b.encode_utf16().collect();
    if a_u16.is_empty() && b_u16.is_empty() {
        return 1.0;
    }
    let diffs = make_diff(&a_u16, &b_u16, false, Duration::from_secs(1));
    let same: usize = diffs
        .iter()
        .filter(|d| d.is_equal())
        .map(|d| d.text().len())
        .sum();
    (2.0 * same as f64) / (a_u16.len() + b_u16.len()) as f64
}

/// Carries `anchor` (in `before`) into `after`, version `version`: through `edits` when known,
/// else through a diff of the two. Without `before`, only the search can place it.
pub fn track(
    anchor: &TrackedRange,
    before: Option<&str>,
    after: &str,
    version: &str,
    edits: Option<&[Edit]>,
) -> TrackedRange {
    let anchor_from = anchor.from as usize;
    let anchor_to = anchor.to as usize;

    let before = match before {
        Some(b) => b,
        None => return search(anchor, after, version, anchor_from, None),
    };
    if before == after {
        return TrackedRange {
            version: version.to_string(),
            ..anchor.clone()
        };
    }

    let computed_edits;
    let edit_list = match edits {
        Some(e) => e,
        None => {
            computed_edits = edits_between(before, after);
            &computed_edits
        }
    };

    let changes = ChangeSet::from_edits(edit_list.to_vec(), before.encode_utf16().count());

    // An insertion right at either end stays outside the target.
    let from = changes.map_pos(anchor_from, 1);
    let to = from.max(changes.map_pos(anchor_to, -1));

    let after_text = Utf16Text::new(after);
    let now = after_text.slice_str(from, to);

    if now == anchor.text && !now.is_empty() {
        return range_in(after, version, from, to, AnchorStatus::Active, Some(&anchor.original));
    }

    // How much of the target the edits deleted, and what they wrote
    let mut deleted: usize = 0;
    let mut written: Vec<(usize, usize)> = Vec::new();
    changes.iter_changed_ranges(|from_a, to_a, from_b, to_b| {
        let overlap_start = from_a.max(anchor_from);
        let overlap_end = to_a.min(anchor_to);
        if overlap_end > overlap_start {
            deleted += overlap_end - overlap_start;
        }
        if to_b > from_b {
            written.push((from_b, to_b));
        }
    });

    let range_len = anchor_to.saturating_sub(anchor_from);
    let kept = if range_len > 0 {
        1.0 - deleted as f64 / range_len as f64
    } else {
        0.0
    };

    if !now.trim().is_empty()
        && kept >= TRACKING.modified
        && similarity(&anchor.text, &now) >= TRACKING.modified
    {
        // Check for a moved block
        if let Some(exact) = moved(anchor, after, &written) {
            return range_in(
                after,
                version,
                exact,
                exact + anchor.text.encode_utf16().count(),
                AnchorStatus::Active,
                Some(&anchor.original),
            );
        }
        let (snapped_from, snapped_to) = snap(after, from, to);
        return range_in(
            after,
            version,
            snapped_from,
            snapped_to,
            AnchorStatus::Modified,
            Some(&anchor.original),
        );
    }
    search(anchor, after, version, from, Some(&written))
}

/// Finds the target by its text when mapping couldn't place it.
pub fn search(
    anchor: &TrackedRange,
    text: &str,
    version: &str,
    near: usize,
    written: Option<&[(usize, usize)]>,
) -> TrackedRange {
    let needle = &anchor.text;
    if needle.trim().is_empty() {
        return TrackedRange {
            version: version.to_string(),
            status: AnchorStatus::Removed,
            ..anchor.clone()
        };
    }

    let text_u16 = Utf16Text::new(text);
    let needle_u16: Vec<u16> = needle.encode_utf16().collect();

    let possible = |from: usize, to: usize| -> bool {
        match written {
            None => true,
            Some(w) => w.iter().any(|&(a, b)| a < to && b > from),
        }
    };

    // Find all exact occurrences
    let mut found: Vec<(usize, f64)> = Vec::new();
    let mut search_start = 0;
    while let Some(at) = text_u16.find_utf16(&needle_u16, search_start) {
        let end = at + needle_u16.len();
        if possible(at, end) {
            let score = context_score(anchor, text, at, end);
            found.push((at, score));
        }
        search_start = at + 1;
    }

    if !found.is_empty() {
        found.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let da = (a.0 as isize - near as isize).unsigned_abs();
                    let db = (b.0 as isize - near as isize).unsigned_abs();
                    da.cmp(&db)
                })
        });
        let (best_from, best_score) = found[0];
        if found.len() > 1 && best_score - found[1].1 <= TRACKING.ambiguous_margin {
            let candidates: Vec<TrackedRangeCandidate> = found
                .iter()
                .filter(|f| best_score - f.1 <= TRACKING.ambiguous_margin)
                .map(|f| TrackedRangeCandidate {
                    from: f.0 as u64,
                    to: (f.0 + needle_u16.len()) as u64,
                })
                .collect();
            return TrackedRange {
                version: version.to_string(),
                status: AnchorStatus::Ambiguous,
                candidates: Some(candidates),
                ..anchor.clone()
            };
        }
        return range_in(
            text,
            version,
            best_from,
            best_from + needle_u16.len(),
            AnchorStatus::Active,
            Some(&anchor.original),
        );
    }

    // Fuzzy match near where it should be
    let head: Vec<u16> = needle_u16.iter().take(32).copied().collect();
    let text_u16_raw: Vec<u16> = text.encode_utf16().collect();
    let loc = near.min(text_u16_raw.len());
    if let Some(at) = match_bitap(
        &text_u16_raw,
        &head,
        loc,
        TRACKING.fuzzy_threshold,
        TRACKING.fuzzy_distance,
    )
        && possible(at, at + needle_u16.len())
    {
        let (aligned_from, aligned_to) = align(needle, text, at);
        if aligned_to > aligned_from {
            let candidate = text_u16.slice_str(aligned_from, aligned_to);
            if similarity(needle, &candidate) >= TRACKING.modified {
                return range_in(
                    text,
                    version,
                    aligned_from,
                    aligned_to,
                    AnchorStatus::Modified,
                    Some(&anchor.original),
                );
            }
        }
    }

    TrackedRange {
        version: version.to_string(),
        status: AnchorStatus::Removed,
        candidates: None,
        ..anchor.clone()
    }
}

/// The one place in `written` holding the target's text unchanged with at least 80% context match.
fn moved(anchor: &TrackedRange, text: &str, written: &[(usize, usize)]) -> Option<usize> {
    let text_u16 = Utf16Text::new(text);
    let needle_u16: Vec<u16> = anchor.text.encode_utf16().collect();
    let mut places: Vec<usize> = Vec::new();
    let mut search_start = 0;
    while let Some(at) = text_u16.find_utf16(&needle_u16, search_start) {
        if places.len() >= 2 {
            break;
        }
        let to = at + needle_u16.len();
        if written.iter().any(|&(a, b)| a < to && b > at) && context_score(anchor, text, at, to) >= 0.8 {
            places.push(at);
        }
        search_start = at + 1;
    }
    if places.len() == 1 {
        Some(places[0])
    } else {
        None
    }
}

/// Where `needle`, found roughly at `at`, starts and ends in `text`.
fn align(needle: &str, text: &str, at: usize) -> (usize, usize) {
    let needle_u16: Vec<u16> = needle.encode_utf16().collect();
    let text_u16 = Utf16Text::new(text);
    let half = needle_u16.len().div_ceil(2);
    let start = at.saturating_sub(half);
    let end = (at + needle_u16.len() + half).min(text_u16.len_utf16());
    let region_str = text_u16.slice_str(start, end);
    let region_u16: Vec<u16> = region_str.encode_utf16().collect();

    let mut diffs = make_diff(&needle_u16, &region_u16, false, Duration::from_secs(1));
    cleanup_semantic(&mut diffs);

    let from_idx = x_index(&diffs, 0);
    let to_idx = if needle_u16.is_empty() {
        from_idx
    } else {
        x_index(&diffs, needle_u16.len() - 1) + 1
    };
    let (mut from, mut to) = snap(text, start + from_idx, start + to_idx);

    // Trim leading lines that make it less similar
    loop {
        if let Some(cut) = text_u16.find_newline(from)
            && cut + 1 < to
        {
            let current = text_u16.slice_str(from, to);
            let trimmed = text_u16.slice_str(cut + 1, to);
            if similarity(needle, &trimmed) >= similarity(needle, &current) {
                from = cut + 1;
                continue;
            }
        }
        break;
    }

    // Trim trailing lines that make it less similar
    loop {
        if let Some(cut) = text_u16.rfind_newline(to)
            && cut > from
        {
            let current = text_u16.slice_str(from, to);
            let trimmed = text_u16.slice_str(from, cut);
            if similarity(needle, &trimmed) >= similarity(needle, &current) {
                to = cut;
                continue;
            }
        }
        break;
    }

    snap(text, from, to)
}

/// A changed range never starts or ends inside a word.
fn snap(text: &str, from: usize, to: usize) -> (usize, usize) {
    let t = Utf16Text::new(text);
    let len = t.len_utf16();
    let mut from = from.min(len);
    let mut to = to.min(len).max(from);

    while from > 0 {
        let prev = t.unit_at(from - 1);
        let curr = t.unit_at(from);
        match (prev, curr) {
            (Some(p), Some(c)) if is_word_char(p) && is_word_char(c) => from -= 1,
            _ => break,
        }
    }
    while to < len {
        let prev = t.unit_at(to - 1);
        let curr = t.unit_at(to);
        match (prev, curr) {
            (Some(p), Some(c)) if is_word_char(p) && is_word_char(c) => to += 1,
            _ => break,
        }
    }
    (from, to)
}

/// 0 to 1: how much of the stored context is still around `from`–`to`.
fn context_score(anchor: &TrackedRange, text: &str, from: usize, to: usize) -> f64 {
    let t = Utf16Text::new(text);
    let prefix_u16: Vec<u16> = anchor.prefix.encode_utf16().collect();
    let suffix_u16: Vec<u16> = anchor.suffix.encode_utf16().collect();

    let before_start = from.saturating_sub(prefix_u16.len());
    let before = t.slice(before_start, from);
    let before_units = before.utf16_units();

    let after_end = (to + suffix_u16.len()).min(t.len_utf16());
    let after = t.slice(to, after_end);
    let after_units = after.utf16_units();

    let mut p = 0;
    while p < prefix_u16.len()
        && p < before_units.len()
        && prefix_u16[prefix_u16.len() - 1 - p] == before_units[before_units.len() - 1 - p]
    {
        p += 1;
    }
    let mut s = 0;
    while s < suffix_u16.len() && s < after_units.len() && suffix_u16[s] == after_units[s] {
        s += 1;
    }
    let total = prefix_u16.len() + suffix_u16.len();
    if total > 0 {
        (p + s) as f64 / total as f64
    } else {
        0.0
    }
}

/// Character offset of a 0-based line and character in `text`.
/// Offsets are UTF-16 code units.
pub fn offset_at(text: &str, line: usize, character: usize) -> usize {
    let t = Utf16Text::new(text);
    let mut at: usize = 0;
    for _ in 0..line {
        match t.find_newline(at) {
            Some(pos) => at = pos + 1,
            None => return t.len_utf16(),
        }
    }
    let end = t.find_newline(at);
    let line_end = match end {
        Some(pos) => pos,
        None => t.len_utf16(),
    };
    (at + character).min(line_end)
}

/// The 0-based line and character of a UTF-16 offset in `text`.
pub fn position_at(text: &str, offset: usize) -> (usize, usize) {
    let t = Utf16Text::new(text);
    let clamped = offset.min(t.len_utf16());
    let up_to = t.slice_str(0, clamped);
    let line = up_to.matches('\n').count();
    let last_nl = up_to.rfind('\n').map(|i| i + 1).unwrap_or(0);
    // Character offset within the line, in UTF-16 units of the slice
    let line_text = &up_to[last_nl..];
    let character = line_text.encode_utf16().count();
    (line, character)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(text: &str, quote: &str) -> TrackedRange {
        let from = text.find(quote).expect("quote not found in text");
        let from_u16 = text[..from].encode_utf16().count();
        let to_u16 = from_u16 + quote.encode_utf16().count();
        range_in(text, "v1", from_u16, to_u16, AnchorStatus::Active, None)
    }

    const FILE: &str = "package main\n\nfunc load(path string) ([]byte, error) {\n\treturn os.ReadFile(path)\n}\n\nfunc save(path string, data []byte) error {\n\treturn os.WriteFile(path, data, 0o644)\n}\n";

    #[test]
    fn lines_inserted_above() {
        let t = target(FILE, "\treturn os.ReadFile(path)");
        let after = FILE.replace("package main\n", "package main\n\nimport \"os\"\n\n// load reads a file.\n");
        let r = track(&t, Some(FILE), &after, "v2", None);
        assert_eq!(r.status, AnchorStatus::Active);
        assert_eq!(&after[byte_range(&after, r.from, r.to)], "\treturn os.ReadFile(path)");
        assert!(r.from > t.from);
    }

    #[test]
    fn name_changed_inside_target() {
        let t = target(FILE, "\treturn os.WriteFile(path, data, 0o644)");
        let after = FILE.replace("os.WriteFile(path, data, 0o644)", "os.WriteFile(name, data, 0o644)");
        let r = track(&t, Some(FILE), &after, "v2", None);
        assert_eq!(r.status, AnchorStatus::Modified);
        assert_eq!(r.original, "\treturn os.WriteFile(path, data, 0o644)");
    }

    #[test]
    fn target_deleted() {
        let func = "func save(path string, data []byte) error {\n\treturn os.WriteFile(path, data, 0o644)\n}\n";
        let t = target(FILE, "\treturn os.WriteFile(path, data, 0o644)");
        let r = track(&t, Some(FILE), &FILE.replace(func, ""), "v2", None);
        assert_eq!(r.status, AnchorStatus::Removed);
    }

    #[test]
    fn function_moved() {
        let func = "func load(path string) ([]byte, error) {\n\treturn os.ReadFile(path)\n}\n\n";
        let t = target(FILE, "\treturn os.ReadFile(path)");
        let after = FILE.replace(func, "").to_string() + "\n" + func;
        let r = track(&t, Some(FILE), &after, "v2", None);
        assert_eq!(r.status, AnchorStatus::Active);
        assert_eq!(&after[byte_range(&after, r.from, r.to)], "\treturn os.ReadFile(path)");
    }

    #[test]
    fn unchanged_file() {
        let t = target(FILE, "\treturn os.ReadFile(path)");
        let r = track(&t, Some(FILE), FILE, "v1", None);
        assert_eq!(r.from, t.from);
        assert_eq!(r.to, t.to);
        assert_eq!(r.status, AnchorStatus::Active);
    }

    #[test]
    fn no_snapshot() {
        let t = target(FILE, "\treturn os.ReadFile(path)");
        let after = format!("// new\n{FILE}");
        let r = track(&t, None, &after, "v2", None);
        assert_eq!(r.status, AnchorStatus::Active);
        assert_eq!(&after[byte_range(&after, r.from, r.to)], "\treturn os.ReadFile(path)");
    }

    #[test]
    fn offset_at_basic() {
        let text = "abc\ndef\nghi";
        assert_eq!(offset_at(text, 0, 0), 0);
        assert_eq!(offset_at(text, 0, 2), 2);
        assert_eq!(offset_at(text, 1, 0), 4);
        assert_eq!(offset_at(text, 1, 2), 6);
        assert_eq!(offset_at(text, 2, 1), 9);
    }

    #[test]
    fn position_at_basic() {
        let text = "abc\ndef\nghi";
        assert_eq!(position_at(text, 0), (0, 0));
        assert_eq!(position_at(text, 2), (0, 2));
        assert_eq!(position_at(text, 4), (1, 0));
        assert_eq!(position_at(text, 6), (1, 2));
    }

    /// Convert UTF-16 from/to offsets to a byte range for slicing a &str.
    fn byte_range(text: &str, from_u16: u64, to_u16: u64) -> std::ops::Range<usize> {
        let from_u16 = from_u16 as usize;
        let to_u16 = to_u16 as usize;
        let mut byte_from = 0;
        let mut byte_to = 0;
        let mut u16_count = 0;
        for (byte_idx, ch) in text.char_indices() {
            if u16_count == from_u16 {
                byte_from = byte_idx;
            }
            u16_count += ch.len_utf16();
            if u16_count == to_u16 {
                byte_to = byte_idx + ch.len_utf8();
                break;
            }
        }
        if u16_count < to_u16 {
            byte_to = text.len();
        }
        byte_from..byte_to
    }
}
