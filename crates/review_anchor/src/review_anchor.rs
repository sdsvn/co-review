//! Comment anchor tracking: carries a comment's target through file edits and classifies
//! what happened to it. Port of `extensions/review/src/common/anchor-tracking.ts`.
//!
//! All offsets are UTF-16 code unit offsets for compatibility with the JavaScript implementation
//! and stored data.

mod dmp;

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

pub use review_model::{AnchorStatus, TrackedRange, TrackedRangeCandidate};

use dmp::{DiffOp, cleanup_semantic, make_diff, match_bitap, x_index};

/// A replacement in the old text: characters `from`--`to` become `insert`.
#[derive(Clone, Debug)]
pub struct Edit {
    pub from: usize,
    pub to: usize,
    pub insert: String,
}

/// Characters of context kept on each side of the target.
const CONTEXT: usize = 40;
/// Modified (not removed) when at least this share of the target's characters survive.
const MODIFIED: f64 = 0.7;
/// Ambiguous when the second-best place scores within this of the best.
const AMBIGUOUS_MARGIN: f64 = 0.1;
/// Fuzzy match threshold (0 = exact, 1 = anything).
const FUZZY_THRESHOLD: f64 = 0.3;
/// How far from the expected place to look for a fuzzy match.
const FUZZY_DISTANCE: usize = 1000;

const NEWLINE: u16 = b'\n' as u16;

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// `units[from..to]`, clamped the way JavaScript's `slice` clamps.
fn clamp(units: &[u16], from: usize, to: usize) -> &[u16] {
    let from = from.min(units.len());
    &units[from..to.min(units.len()).max(from)]
}

/// JavaScript's `slice` over UTF-16 code units.
pub fn slice(units: &[u16], from: usize, to: usize) -> String {
    String::from_utf16_lossy(clamp(units, from, to))
}

/// JavaScript's `indexOf` over UTF-16 code units.
pub fn index_of(haystack: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from.min(haystack.len()));
    }
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| from + at)
}

fn next_newline(units: &[u16], from: usize) -> Option<usize> {
    units[from..]
        .iter()
        .position(|&u| u == NEWLINE)
        .map(|at| from + at)
}

/// JavaScript's `/[\p{L}\p{N}_]/u` on one UTF-16 unit (so never a surrogate half).
fn is_word_char(unit: u16) -> bool {
    static WORD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[\p{L}\p{N}_]$").expect("static regex"));
    char::from_u32(unit.into()).is_some_and(|c| WORD.is_match(c.encode_utf8(&mut [0; 4])))
}

/// A target in `text` from `from` to `to`: its text and context, in version `version`.
pub fn range_in(
    text: &str,
    version: &str,
    from: usize,
    to: usize,
    status: AnchorStatus,
    original: Option<&str>,
) -> TrackedRange {
    range_at(&units(text), version, from, to, status, original)
}

fn range_at(
    t: &[u16],
    version: &str,
    from: usize,
    to: usize,
    status: AnchorStatus,
    original: Option<&str>,
) -> TrackedRange {
    let target = slice(t, from, to);
    TrackedRange {
        version: version.to_string(),
        from: from as u64,
        to: to as u64,
        prefix: slice(t, from.saturating_sub(CONTEXT), from),
        suffix: slice(t, to, to + CONTEXT),
        original: original.map_or_else(|| target.clone(), str::to_string),
        text: target,
        status,
        candidates: None,
        extra: Default::default(),
    }
}

/// The edits that turn `before` into `after`: a line diff first, then characters within
/// changed lines, semantically cleaned up.
pub fn edits_between(before: &str, after: &str) -> Vec<Edit> {
    let mut diffs = make_diff(&units(before), &units(after), true, Duration::from_secs(2));
    cleanup_semantic(&mut diffs);

    let mut edits = Vec::new();
    let mut at = 0;
    let mut diffs = diffs.iter().peekable();
    while let Some(diff) = diffs.next() {
        if diff.is_equal() {
            at += diff.text().len();
            continue;
        }
        let mut removed = 0;
        let mut inserted = Vec::new();
        let mut next = Some(diff);
        while let Some(diff) = next {
            match diff {
                DiffOp::Delete(t) => removed += t.len(),
                DiffOp::Insert(t) => inserted.extend_from_slice(t),
                DiffOp::Equal(_) => {}
            }
            next = diffs.next_if(|d| !d.is_equal());
        }
        edits.push(Edit {
            from: at,
            to: at + removed,
            insert: String::from_utf16_lossy(&inserted),
        });
        at += removed;
    }
    edits
}

/// How alike two texts are, 0 to 1: the share of their characters in common.
pub fn similarity(a: &str, b: &str) -> f64 {
    let (a, b) = (units(a), units(b));
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let same: usize = make_diff(&a, &b, false, Duration::from_secs(1))
        .iter()
        .filter(|d| d.is_equal())
        .map(|d| d.text().len())
        .sum();
    (2.0 * same as f64) / (a.len() + b.len()) as f64
}

/// Where `pos` in the old text lands after `edits`. An insertion right at `pos` goes before it
/// when `assoc > 0` and after it when `assoc < 0`, as in CodeMirror's `mapPos`.
fn map_pos(edits: &[Edit], pos: usize, assoc: i32) -> usize {
    let mut shift: isize = 0;
    for edit in edits {
        let insertion = edit.from == edit.to;
        let inserted = edit.insert.encode_utf16().count() as isize;
        if edit.from > pos || (edit.from == pos && assoc < 0 && insertion) {
            break;
        }
        if edit.from == pos && insertion {
            shift += inserted;
        } else if edit.to <= pos {
            shift += inserted - (edit.to as isize - edit.from as isize);
        } else {
            let end = if assoc < 0 { 0 } else { inserted };
            return (edit.from as isize + shift + end) as usize;
        }
    }
    (pos as isize + shift) as usize
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
    let after_units = units(after);

    let Some(before) = before else {
        return search(anchor, &after_units, version, anchor_from, None);
    };
    if before == after {
        return TrackedRange {
            version: version.to_string(),
            ..anchor.clone()
        };
    }

    let computed;
    let edits = match edits {
        Some(edits) => edits,
        None => {
            computed = edits_between(before, after);
            &computed
        }
    };

    // An insertion right at either end stays outside the target.
    let from = map_pos(edits, anchor_from, 1);
    let to = from.max(map_pos(edits, anchor_to, -1));
    let now = slice(&after_units, from, to);
    let place = |from, to, status| {
        range_at(
            &after_units,
            version,
            from,
            to,
            status,
            Some(&anchor.original),
        )
    };

    if now == anchor.text && !now.is_empty() {
        return place(from, to, AnchorStatus::Active);
    }

    // How much of the target the edits deleted, and where in `after` they wrote.
    let mut deleted = 0;
    let mut written = Vec::new();
    let mut shift: isize = 0;
    for edit in edits {
        let inserted = edit.insert.encode_utf16().count();
        deleted += edit
            .to
            .min(anchor_to)
            .saturating_sub(edit.from.max(anchor_from));
        let written_from = (edit.from as isize + shift) as usize;
        if inserted > 0 {
            written.push((written_from, written_from + inserted));
        }
        shift += inserted as isize - (edit.to as isize - edit.from as isize);
    }

    let range_len = anchor_to.saturating_sub(anchor_from);
    let kept = if range_len > 0 {
        1.0 - deleted as f64 / range_len as f64
    } else {
        0.0
    };

    if !now.trim().is_empty() && kept >= MODIFIED && similarity(&anchor.text, &now) >= MODIFIED {
        if let Some(exact) = moved(anchor, &after_units, &written) {
            let exact_to = exact + anchor.text.encode_utf16().count();
            return place(exact, exact_to, AnchorStatus::Active);
        }
        let (from, to) = snap(&after_units, from, to);
        return place(from, to, AnchorStatus::Modified);
    }
    search(anchor, &after_units, version, from, Some(&written))
}

/// Finds the target by its text when mapping couldn't place it.
fn search(
    anchor: &TrackedRange,
    text_units: &[u16],
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
    let needle_units = units(needle);
    let place = |from, to, status| {
        range_at(
            text_units,
            version,
            from,
            to,
            status,
            Some(&anchor.original),
        )
    };
    let possible = |from: usize, to: usize| {
        written.is_none_or(|written| written.iter().any(|&(a, b)| a < to && b > from))
    };

    let mut found: Vec<(usize, f64)> = Vec::new();
    let mut start = 0;
    while let Some(at) = index_of(text_units, &needle_units, start) {
        let end = at + needle_units.len();
        if possible(at, end) {
            found.push((at, context_score(anchor, text_units, at, end)));
        }
        start = at + 1;
    }

    if !found.is_empty() {
        found.sort_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then_with(|| a.0.abs_diff(near).cmp(&b.0.abs_diff(near)))
        });
        let (best_from, best_score) = found[0];
        if found.len() > 1 && best_score - found[1].1 <= AMBIGUOUS_MARGIN {
            let candidates = found
                .iter()
                .filter(|f| best_score - f.1 <= AMBIGUOUS_MARGIN)
                .map(|f| TrackedRangeCandidate {
                    from: f.0 as u64,
                    to: (f.0 + needle_units.len()) as u64,
                })
                .collect();
            return TrackedRange {
                version: version.to_string(),
                status: AnchorStatus::Ambiguous,
                candidates: Some(candidates),
                ..anchor.clone()
            };
        }
        let best_to = best_from + needle_units.len();
        return place(best_from, best_to, AnchorStatus::Active);
    }

    // Fuzzy match near where it should be.
    let head = &needle_units[..needle_units.len().min(32)];
    let loc = near.min(text_units.len());
    if let Some(at) = match_bitap(text_units, head, loc, FUZZY_THRESHOLD, FUZZY_DISTANCE)
        && possible(at, at + needle_units.len())
    {
        let (from, to) = align(needle, text_units, at);
        if to > from && similarity(needle, &slice(text_units, from, to)) >= MODIFIED {
            return place(from, to, AnchorStatus::Modified);
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
fn moved(anchor: &TrackedRange, text: &[u16], written: &[(usize, usize)]) -> Option<usize> {
    let needle = units(&anchor.text);
    let mut places = Vec::new();
    let mut start = 0;
    while places.len() < 2
        && let Some(at) = index_of(text, &needle, start)
    {
        let to = at + needle.len();
        if written.iter().any(|&(a, b)| a < to && b > at)
            && context_score(anchor, text, at, to) >= 0.8
        {
            places.push(at);
        }
        start = at + 1;
    }
    match places[..] {
        [at] => Some(at),
        _ => None,
    }
}

/// Where `needle`, found roughly at `at`, starts and ends in `text`.
fn align(needle: &str, text: &[u16], at: usize) -> (usize, usize) {
    let needle_units = units(needle);
    let half = needle_units.len().div_ceil(2);
    let start = at.saturating_sub(half);
    let region = units(&slice(text, start, at + needle_units.len() + half));

    let mut diffs = make_diff(&needle_units, &region, false, Duration::from_secs(1));
    cleanup_semantic(&mut diffs);

    let from_idx = x_index(&diffs, 0);
    let to_idx = match needle_units.len() {
        0 => from_idx,
        len => x_index(&diffs, len - 1) + 1,
    };
    let (mut from, mut to) = snap(text, start + from_idx, start + to_idx);
    let improves = |from: usize, to: usize, current: (usize, usize)| {
        similarity(needle, &slice(text, from, to))
            >= similarity(needle, &slice(text, current.0, current.1))
    };

    // Trim leading, then trailing lines that make it less similar.
    while let Some(cut) = next_newline(text, from)
        && cut + 1 < to
        && improves(cut + 1, to, (from, to))
    {
        from = cut + 1;
    }
    while let Some(cut) = text[..to.min(text.len())]
        .iter()
        .rposition(|&u| u == NEWLINE)
        && cut > from
        && improves(from, cut, (from, to))
    {
        to = cut;
    }

    snap(text, from, to)
}

/// A changed range never starts or ends inside a word.
fn snap(text: &[u16], from: usize, to: usize) -> (usize, usize) {
    let inside_word = |at: usize| {
        at > 0 && at < text.len() && is_word_char(text[at - 1]) && is_word_char(text[at])
    };
    let mut from = from.min(text.len());
    let mut to = to.min(text.len()).max(from);
    while inside_word(from) {
        from -= 1;
    }
    while inside_word(to) {
        to += 1;
    }
    (from, to)
}

/// 0 to 1: how much of the stored context is still around `from`–`to`.
fn context_score(anchor: &TrackedRange, text: &[u16], from: usize, to: usize) -> f64 {
    let prefix = units(&anchor.prefix);
    let suffix = units(&anchor.suffix);
    let total = prefix.len() + suffix.len();
    if total == 0 {
        return 0.0;
    }
    let before = clamp(text, from.saturating_sub(prefix.len()), from);
    let after = clamp(text, to, to + suffix.len());
    let p = prefix
        .iter()
        .rev()
        .zip(before.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let s = suffix.iter().zip(after).take_while(|(a, b)| a == b).count();
    (p + s) as f64 / total as f64
}

/// Character offset of a 0-based line and character in `text`.
pub fn offset_at(text: &str, line: usize, character: usize) -> usize {
    let t = units(text);
    let mut at = 0;
    for _ in 0..line {
        match next_newline(&t, at) {
            Some(newline) => at = newline + 1,
            None => return t.len(),
        }
    }
    (at + character).min(next_newline(&t, at).unwrap_or(t.len()))
}

/// The 0-based line and character of a UTF-16 offset in `text`.
pub fn position_at(text: &str, offset: usize) -> (usize, usize) {
    let up_to = slice(&units(text), 0, offset);
    let line_start = up_to.rfind('\n').map_or(0, |i| i + 1);
    (
        up_to.matches('\n').count(),
        up_to[line_start..].encode_utf16().count(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_of_and_slice_match_javascript() {
        let text = units("aé😀b");
        assert_eq!(index_of(&text, &units("b"), 0), Some(4));
        assert_eq!(index_of(&text, &units(""), 2), Some(2));
        assert_eq!(index_of(&text, &units(""), 9), Some(5));
        assert_eq!(slice(&text, 1, 4), "é😀");
    }

    /// The `track` cases of fixtures/dmp.json, recorded from the Theia app's anchor-tracking.js.
    #[test]
    fn snaps_to_words_as_javascript_does() {
        let cases: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../tests/fixtures/dmp.json")).expect("dmp.json");
        let tracked = |value: &serde_json::Value| {
            serde_json::from_value::<TrackedRange>(value.clone()).expect("tracked range")
        };
        for case in cases.iter().filter(|case| case["input"]["type"] == "track") {
            let input = &case["input"];
            let text = |key: &str| input[key].as_str().expect("text input");
            let got = track(
                &tracked(&input["anchor"]),
                Some(text("before")),
                text("after"),
                text("version"),
                None,
            );
            let want = tracked(&case["expected"]);
            assert_eq!(
                (got.status, got.from, got.to, got.text),
                (want.status, want.from, want.to, want.text),
                "{}",
                case["name"]
            );
        }
    }
}
