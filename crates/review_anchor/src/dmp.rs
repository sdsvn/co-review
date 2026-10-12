//! The parts of @sanity/diff-match-patch anchor tracking uses (`makeDiff` with checkLines,
//! `cleanupSemantic`, `match` and `xIndex`), on UTF-16 code units as JavaScript strings are.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::index_of;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffOp {
    Equal(Vec<u16>),
    Insert(Vec<u16>),
    Delete(Vec<u16>),
}

impl DiffOp {
    pub fn text(&self) -> &[u16] {
        match self {
            DiffOp::Equal(t) | DiffOp::Insert(t) | DiffOp::Delete(t) => t,
        }
    }

    pub fn is_equal(&self) -> bool {
        matches!(self, DiffOp::Equal(_))
    }

    fn with_text(&self, text: Vec<u16>) -> Self {
        match self {
            DiffOp::Equal(_) => DiffOp::Equal(text),
            DiffOp::Insert(_) => DiffOp::Insert(text),
            DiffOp::Delete(_) => DiffOp::Delete(text),
        }
    }
}

/// Compute the diff between two UTF-16 texts. When `check_lines` is true, does a line-level
/// diff first (as git does), then refines character-level within changed runs.
pub fn make_diff(
    text1: &[u16],
    text2: &[u16],
    check_lines: bool,
    timeout: Duration,
) -> Vec<DiffOp> {
    if text1 == text2 {
        if text1.is_empty() {
            return vec![];
        }
        return vec![DiffOp::Equal(text1.to_vec())];
    }
    if text1.is_empty() {
        return vec![DiffOp::Insert(text2.to_vec())];
    }
    if text2.is_empty() {
        return vec![DiffOp::Delete(text1.to_vec())];
    }

    let common_prefix_len = common_prefix(text1, text2);
    let prefix = text1[..common_prefix_len].to_vec();
    let text1 = &text1[common_prefix_len..];
    let text2 = &text2[common_prefix_len..];

    let common_suffix_len = common_suffix(text1, text2);
    let suffix = text1[text1.len() - common_suffix_len..].to_vec();
    let text1 = &text1[..text1.len() - common_suffix_len];
    let text2 = &text2[..text2.len() - common_suffix_len];

    let deadline = Instant::now() + timeout;

    let mut diffs = if check_lines && text1.len() > 100 && text2.len() > 100 {
        diff_lines_then_chars(text1, text2, deadline)
    } else {
        diff_compute(text1, text2, deadline)
    };

    if !prefix.is_empty() {
        diffs.insert(0, DiffOp::Equal(prefix));
    }
    if !suffix.is_empty() {
        diffs.push(DiffOp::Equal(suffix));
    }

    merge_diffs(&mut diffs);
    diffs
}

/// Line-level diff then character refinement within changed hunks.
fn diff_lines_then_chars(text1: &[u16], text2: &[u16], deadline: Instant) -> Vec<DiffOp> {
    let (encoded1, encoded2, line_array) = lines_to_chars(text1, text2);
    let mut diffs = diff_compute(&encoded1, &encoded2, deadline);

    for diff in &mut diffs {
        let text = diff
            .text()
            .iter()
            .flat_map(|&c| &line_array[c as usize])
            .copied()
            .collect();
        *diff = diff.with_text(text);
    }

    merge_diffs(&mut diffs);
    let mut result = Vec::new();
    let mut i = 0;
    while i < diffs.len() {
        if diffs[i].is_equal() {
            result.push(diffs[i].clone());
            i += 1;
            continue;
        }
        let mut del_text = Vec::new();
        let mut ins_text = Vec::new();
        while i < diffs.len() && !diffs[i].is_equal() {
            match &diffs[i] {
                DiffOp::Delete(t) => del_text.extend_from_slice(t),
                DiffOp::Insert(t) => ins_text.extend_from_slice(t),
                _ => {}
            }
            i += 1;
        }
        if !del_text.is_empty() && !ins_text.is_empty() {
            let sub = diff_compute(&del_text, &ins_text, deadline);
            result.extend(sub);
        } else if !del_text.is_empty() {
            result.push(DiffOp::Delete(del_text));
        } else if !ins_text.is_empty() {
            result.push(DiffOp::Insert(ins_text));
        }
    }
    result
}

/// Convert each line to a single character for line-level diffing.
fn lines_to_chars(text1: &[u16], text2: &[u16]) -> (Vec<u16>, Vec<u16>, Vec<Vec<u16>>) {
    // Index 0 unused, as in diff-match-patch.
    let mut line_array: Vec<Vec<u16>> = vec![vec![]];
    let mut line_hash: HashMap<Vec<u16>, u16> = HashMap::new();

    let encode = |text: &[u16],
                  line_array: &mut Vec<Vec<u16>>,
                  line_hash: &mut HashMap<Vec<u16>, u16>|
     -> Vec<u16> {
        let mut chars = Vec::new();
        let mut start = 0;
        while start < text.len() {
            let end = text[start..]
                .iter()
                .position(|&c| c == b'\n' as u16)
                .map(|p| start + p + 1)
                .unwrap_or(text.len());
            let line = &text[start..end];
            let idx = *line_hash.entry(line.to_vec()).or_insert_with(|| {
                line_array.push(line.to_vec());
                (line_array.len() - 1) as u16
            });
            chars.push(idx);
            start = end;
        }
        chars
    };

    let chars1 = encode(text1, &mut line_array, &mut line_hash);
    let chars2 = encode(text2, &mut line_array, &mut line_hash);
    (chars1, chars2, line_array)
}

fn diff_compute(text1: &[u16], text2: &[u16], deadline: Instant) -> Vec<DiffOp> {
    if text1.is_empty() {
        return vec![DiffOp::Insert(text2.to_vec())];
    }
    if text2.is_empty() {
        return vec![DiffOp::Delete(text1.to_vec())];
    }

    if let Some(pos) = index_of(text1, text2, 0) {
        return vec![
            DiffOp::Delete(text1[..pos].to_vec()),
            DiffOp::Equal(text2.to_vec()),
            DiffOp::Delete(text1[pos + text2.len()..].to_vec()),
        ]
        .into_iter()
        .filter(|d| !d.text().is_empty())
        .collect();
    }

    if let Some(pos) = index_of(text2, text1, 0) {
        return vec![
            DiffOp::Insert(text2[..pos].to_vec()),
            DiffOp::Equal(text1.to_vec()),
            DiffOp::Insert(text2[pos + text1.len()..].to_vec()),
        ]
        .into_iter()
        .filter(|d| !d.text().is_empty())
        .collect();
    }

    if text1.len() == 1 || text2.len() == 1 {
        return vec![
            DiffOp::Delete(text1.to_vec()),
            DiffOp::Insert(text2.to_vec()),
        ];
    }

    if let Some((prefix1, suffix1, prefix2, suffix2, common)) = half_match(text1, text2) {
        let mut result = diff_compute(&prefix1, &prefix2, deadline);
        result.push(DiffOp::Equal(common));
        result.extend(diff_compute(&suffix1, &suffix2, deadline));
        return result;
    }

    diff_bisect(text1, text2, deadline)
}

/// Myers' bisect diff algorithm.
fn diff_bisect(text1: &[u16], text2: &[u16], deadline: Instant) -> Vec<DiffOp> {
    let text1_len = text1.len() as isize;
    let text2_len = text2.len() as isize;
    let max_d = ((text1_len + text2_len + 1) / 2) as usize;
    let v_offset = max_d;
    let v_length = 2 * max_d;

    let mut v1 = vec![-1isize; v_length + 2];
    let mut v2 = vec![-1isize; v_length + 2];
    v1[v_offset + 1] = 0;
    v2[v_offset + 1] = 0;

    let delta = text1_len - text2_len;
    let front = delta % 2 != 0;

    for d in 0..=max_d {
        if Instant::now() > deadline {
            break;
        }

        // Forward
        let d_i = d as isize;
        let mut k = -(d_i);
        while k <= d_i {
            let k_offset = (v_offset as isize + k) as usize;
            let mut x1: isize;
            if k == -d_i || (k != d_i && v1[k_offset - 1] < v1[k_offset + 1]) {
                x1 = v1[k_offset + 1];
            } else {
                x1 = v1[k_offset - 1] + 1;
            }
            let mut y1 = x1 - k;
            while x1 < text1_len && y1 < text2_len && text1[x1 as usize] == text2[y1 as usize] {
                x1 += 1;
                y1 += 1;
            }
            v1[k_offset] = x1;
            if x1 > text1_len {
                k += 2;
                continue;
            }
            if y1 > text2_len {
                k += 2;
                continue;
            }
            if front {
                let k2_offset = v_offset as isize + delta - k;
                if k2_offset >= 0 && (k2_offset as usize) < v2.len() && v2[k2_offset as usize] != -1
                {
                    let x2 = text1_len - v2[k2_offset as usize];
                    if x1 >= x2 {
                        return diff_bisect_split(text1, text2, x1 as usize, y1 as usize, deadline);
                    }
                }
            }
            k += 2;
        }

        // Reverse
        k = -(d_i);
        while k <= d_i {
            let k_offset = (v_offset as isize + k) as usize;
            let mut x1: isize;
            if k == -d_i || (k != d_i && v2[k_offset - 1] < v2[k_offset + 1]) {
                x1 = v2[k_offset + 1];
            } else {
                x1 = v2[k_offset - 1] + 1;
            }
            let mut y1 = x1 - k;
            while x1 < text1_len
                && y1 < text2_len
                && text1[(text1_len - x1 - 1) as usize] == text2[(text2_len - y1 - 1) as usize]
            {
                x1 += 1;
                y1 += 1;
            }
            v2[k_offset] = x1;
            if x1 > text1_len {
                k += 2;
                continue;
            }
            if y1 > text2_len {
                k += 2;
                continue;
            }
            if !front {
                let k2_offset = v_offset as isize + delta - k;
                if k2_offset >= 0 && (k2_offset as usize) < v1.len() && v1[k2_offset as usize] != -1
                {
                    let x2 = v1[k2_offset as usize];
                    let y2 = v_offset as isize + x2 - k2_offset;
                    let x1_real = text1_len - x1;
                    if x2 >= x1_real {
                        return diff_bisect_split(text1, text2, x2 as usize, y2 as usize, deadline);
                    }
                }
            }
            k += 2;
        }
    }
    // Timeout or no solution: return delete+insert
    vec![
        DiffOp::Delete(text1.to_vec()),
        DiffOp::Insert(text2.to_vec()),
    ]
}

fn diff_bisect_split(
    text1: &[u16],
    text2: &[u16],
    x: usize,
    y: usize,
    deadline: Instant,
) -> Vec<DiffOp> {
    let mut diffs = diff_compute(&text1[..x], &text2[..y], deadline);
    diffs.extend(diff_compute(&text1[x..], &text2[y..], deadline));
    diffs
}

/// Five-tuple returned by `half_match`: (text1_prefix, text1_suffix, text2_prefix, text2_suffix, common_middle).
type HalfMatch = (Vec<u16>, Vec<u16>, Vec<u16>, Vec<u16>, Vec<u16>);

/// Find a shared substring that is at least half the length of the longer text.
fn half_match(text1: &[u16], text2: &[u16]) -> Option<HalfMatch> {
    let longtext = if text1.len() > text2.len() {
        text1
    } else {
        text2
    };
    let shorttext = if text1.len() > text2.len() {
        text2
    } else {
        text1
    };
    if longtext.len() < 4 || shorttext.len() * 2 < longtext.len() {
        return None;
    }

    let hm1 = half_match_inner(longtext, shorttext, longtext.len().div_ceil(4));
    let hm2 = half_match_inner(longtext, shorttext, longtext.len().div_ceil(2));
    let hm = match (hm1, hm2) {
        (None, None) => return None,
        (Some(h), None) | (None, Some(h)) => h,
        (Some(h1), Some(h2)) => {
            if h1.4.len() > h2.4.len() {
                h1
            } else {
                h2
            }
        }
    };

    if text1.len() > text2.len() {
        Some(hm)
    } else {
        Some((hm.2, hm.3, hm.0, hm.1, hm.4))
    }
}

fn half_match_inner(longtext: &[u16], shorttext: &[u16], i: usize) -> Option<HalfMatch> {
    let seed = &longtext[i..i + longtext.len() / 4];
    let mut best = (vec![], vec![], vec![], vec![], vec![]);
    let mut j_opt = index_of(shorttext, seed, 0);
    while let Some(j) = j_opt {
        let prefix_len = common_prefix(&longtext[i..], &shorttext[j..]);
        let suffix_len = common_suffix(&longtext[..i], &shorttext[..j]);
        if best.4.len() < suffix_len + prefix_len {
            best = (
                longtext[..i - suffix_len].to_vec(),
                longtext[i + prefix_len..].to_vec(),
                shorttext[..j - suffix_len].to_vec(),
                shorttext[j + prefix_len..].to_vec(),
                longtext[i - suffix_len..i + prefix_len].to_vec(),
            );
        }
        j_opt = index_of(shorttext, seed, j + 1);
    }
    if best.4.len() * 2 >= longtext.len() {
        Some(best)
    } else {
        None
    }
}

fn common_prefix(text1: &[u16], text2: &[u16]) -> usize {
    text1.iter().zip(text2).take_while(|(a, b)| a == b).count()
}

fn common_suffix(text1: &[u16], text2: &[u16]) -> usize {
    text1
        .iter()
        .rev()
        .zip(text2.iter().rev())
        .take_while(|(a, b)| a == b)
        .count()
}

/// DMP cleanupMerge: collects consecutive deletes and inserts into canonical form
/// (all deletes before all inserts in each run), factoring out common prefix/suffix
/// between the delete and insert text.
fn merge_diffs(diffs: &mut Vec<DiffOp>) {
    diffs.push(DiffOp::Equal(vec![]));
    let mut pointer = 0;
    let mut count_delete = 0usize;
    let mut count_insert = 0usize;
    let mut text_delete: Vec<u16> = Vec::new();
    let mut text_insert: Vec<u16> = Vec::new();

    while pointer < diffs.len() {
        match &diffs[pointer] {
            DiffOp::Insert(t) => {
                count_insert += 1;
                text_insert.extend_from_slice(t);
                pointer += 1;
            }
            DiffOp::Delete(t) => {
                count_delete += 1;
                text_delete.extend_from_slice(t);
                pointer += 1;
            }
            DiffOp::Equal(_) => {
                if count_delete + count_insert > 1 {
                    if count_delete != 0 && count_insert != 0 {
                        let cp = common_prefix(&text_insert, &text_delete);
                        if cp > 0 {
                            let base = pointer - count_delete - count_insert;
                            if base > 0 && diffs[base - 1].is_equal() {
                                let mut eq = diffs[base - 1].text().to_vec();
                                eq.extend_from_slice(&text_insert[..cp]);
                                diffs[base - 1] = DiffOp::Equal(eq);
                            } else {
                                diffs.insert(0, DiffOp::Equal(text_insert[..cp].to_vec()));
                                pointer += 1;
                            }
                            text_insert = text_insert[cp..].to_vec();
                            text_delete = text_delete[cp..].to_vec();
                        }
                        let cs = common_suffix(&text_insert, &text_delete);
                        if cs > 0 {
                            let mut eq = text_insert[text_insert.len() - cs..].to_vec();
                            eq.extend_from_slice(diffs[pointer].text());
                            diffs[pointer] = DiffOp::Equal(eq);
                            text_insert = text_insert[..text_insert.len() - cs].to_vec();
                            text_delete = text_delete[..text_delete.len() - cs].to_vec();
                        }
                    }
                    pointer -= count_delete + count_insert;
                    diffs.drain(pointer..pointer + count_delete + count_insert);
                    if !text_delete.is_empty() {
                        diffs.insert(pointer, DiffOp::Delete(text_delete.clone()));
                        pointer += 1;
                    }
                    if !text_insert.is_empty() {
                        diffs.insert(pointer, DiffOp::Insert(text_insert.clone()));
                        pointer += 1;
                    }
                    pointer += 1;
                } else if pointer != 0 && diffs[pointer - 1].is_equal() {
                    let mut eq = diffs[pointer - 1].text().to_vec();
                    eq.extend_from_slice(diffs[pointer].text());
                    diffs[pointer - 1] = DiffOp::Equal(eq);
                    diffs.remove(pointer);
                } else {
                    pointer += 1;
                }
                count_insert = 0;
                count_delete = 0;
                text_delete.clear();
                text_insert.clear();
            }
        }
    }
    if diffs.last().is_some_and(|d| d.text().is_empty()) {
        diffs.pop();
    }

    // Slide single edits surrounded by equalities sideways to eliminate an equality.
    let mut has_changes = false;
    pointer = 1;
    while pointer + 1 < diffs.len() {
        if diffs[pointer - 1].is_equal() && diffs[pointer + 1].is_equal() {
            let edit_text = diffs[pointer].text().to_vec();
            let prev_text = diffs[pointer - 1].text().to_vec();
            let next_text = diffs[pointer + 1].text().to_vec();

            if edit_text.len() >= prev_text.len()
                && edit_text[edit_text.len() - prev_text.len()..] == prev_text[..]
            {
                let mut new_edit = prev_text.clone();
                new_edit.extend_from_slice(&edit_text[..edit_text.len() - prev_text.len()]);
                diffs[pointer] = diffs[pointer].with_text(new_edit);
                let mut new_next = prev_text;
                new_next.extend_from_slice(&next_text);
                diffs[pointer + 1] = DiffOp::Equal(new_next);
                diffs.remove(pointer - 1);
                has_changes = true;
            } else if edit_text.len() >= next_text.len()
                && edit_text[..next_text.len()] == next_text[..]
            {
                let mut new_prev = prev_text;
                new_prev.extend_from_slice(&next_text);
                diffs[pointer - 1] = DiffOp::Equal(new_prev);
                let mut new_edit = edit_text[next_text.len()..].to_vec();
                new_edit.extend_from_slice(&next_text);
                diffs[pointer] = diffs[pointer].with_text(new_edit);
                diffs.remove(pointer + 1);
                has_changes = true;
            }
        }
        pointer += 1;
    }
    if has_changes {
        merge_diffs(diffs);
    }
}

/// diff-match-patch's `cleanupSemantic`: make the diffs read like edits.
pub fn cleanup_semantic(diffs: &mut Vec<DiffOp>) {
    if diffs.is_empty() {
        return;
    }

    let mut equalities: Vec<usize> = Vec::new();
    let mut last_equality: Option<Vec<u16>> = None;
    let mut pointer: usize = 0;
    let mut length_insertions1: usize = 0;
    let mut length_deletions1: usize = 0;
    let mut length_insertions2: usize = 0;
    let mut length_deletions2: usize = 0;

    while pointer < diffs.len() {
        if diffs[pointer].is_equal() {
            equalities.push(pointer);
            length_insertions1 = length_insertions2;
            length_deletions1 = length_deletions2;
            length_insertions2 = 0;
            length_deletions2 = 0;
            last_equality = Some(diffs[pointer].text().to_vec());
        } else {
            match &diffs[pointer] {
                DiffOp::Insert(t) => length_insertions2 += t.len(),
                DiffOp::Delete(t) => length_deletions2 += t.len(),
                _ => {}
            }
            if let Some(ref eq) = last_equality
                && eq.len() <= length_insertions1.max(length_deletions1)
                && eq.len() <= length_insertions2.max(length_deletions2)
            {
                // Split the equality into a delete and an insert.
                let insert_at = equalities[equalities.len() - 1];
                diffs.insert(insert_at, DiffOp::Delete(eq.clone()));
                diffs[insert_at + 1] = DiffOp::Insert(diffs[insert_at + 1].text().to_vec());
                equalities.pop();
                equalities.pop();
                pointer = equalities.last().copied().unwrap_or(0);
                length_insertions1 = 0;
                length_deletions1 = 0;
                length_insertions2 = 0;
                length_deletions2 = 0;
                last_equality = None;
                continue;
            }
        }
        pointer += 1;
    }

    merge_diffs(diffs);
    cleanup_semantic_lossless(diffs);

    // Find overlaps between deletions and insertions.
    let mut pointer = 1;
    while pointer < diffs.len() {
        if matches!(diffs[pointer - 1], DiffOp::Delete(_))
            && matches!(diffs[pointer], DiffOp::Insert(_))
        {
            let deletion = diffs[pointer - 1].text().to_vec();
            let insertion = diffs[pointer].text().to_vec();
            let overlap_len1 = overlap(&deletion, &insertion);
            let overlap_len2 = overlap(&insertion, &deletion);
            if overlap_len1 >= overlap_len2 {
                if overlap_len1 as f64 >= deletion.len() as f64 / 2.0
                    || overlap_len1 as f64 >= insertion.len() as f64 / 2.0
                {
                    diffs.insert(pointer, DiffOp::Equal(insertion[..overlap_len1].to_vec()));
                    diffs[pointer - 1] =
                        DiffOp::Delete(deletion[..deletion.len() - overlap_len1].to_vec());
                    diffs[pointer + 1] = DiffOp::Insert(insertion[overlap_len1..].to_vec());
                    pointer += 1;
                }
            } else if overlap_len2 as f64 >= deletion.len() as f64 / 2.0
                || overlap_len2 as f64 >= insertion.len() as f64 / 2.0
            {
                diffs.insert(pointer, DiffOp::Equal(deletion[..overlap_len2].to_vec()));
                diffs[pointer - 1] =
                    DiffOp::Insert(insertion[..insertion.len() - overlap_len2].to_vec());
                diffs[pointer + 1] = DiffOp::Delete(deletion[overlap_len2..].to_vec());
                pointer += 1;
            }
            pointer += 1;
        }
        pointer += 1;
    }

    diffs.retain(|d| !d.text().is_empty());
}

fn cleanup_semantic_lossless(diffs: &mut Vec<DiffOp>) {
    let mut pointer = 1;
    while pointer + 1 < diffs.len() {
        if diffs[pointer - 1].is_equal() && diffs[pointer + 1].is_equal() {
            let eq1_orig = diffs[pointer - 1].text().to_vec();
            let edit_orig = diffs[pointer].text().to_vec();
            let eq2_orig = diffs[pointer + 1].text().to_vec();

            // Shift the edit to absorb common suffix with the preceding equality
            let cs = common_suffix(&eq1_orig, &edit_orig);
            let (mut equality1, mut edit, mut equality2);
            if cs > 0 {
                let common = edit_orig[edit_orig.len() - cs..].to_vec();
                equality1 = eq1_orig[..eq1_orig.len() - cs].to_vec();
                let mut new_edit = common.clone();
                new_edit.extend_from_slice(&edit_orig[..edit_orig.len() - cs]);
                edit = new_edit;
                let mut new_eq2 = common;
                new_eq2.extend_from_slice(&eq2_orig);
                equality2 = new_eq2;
            } else {
                equality1 = eq1_orig;
                edit = edit_orig;
                equality2 = eq2_orig;
            }

            // Slide forward through equality2 to find the best boundary alignment
            let mut best_score = cleanup_semantic_score(&equality1, &edit)
                + cleanup_semantic_score(&edit, &equality2);
            let mut best_eq1 = equality1.clone();
            let mut best_edit = edit.clone();
            let mut best_eq2 = equality2.clone();

            while !equality2.is_empty() && !edit.is_empty() && equality2[0] == edit[0] {
                equality1.push(edit[0]);
                let mut new_ed = edit[1..].to_vec();
                new_ed.push(equality2[0]);
                equality2 = equality2[1..].to_vec();
                edit = new_ed;
                let score = cleanup_semantic_score(&equality1, &edit)
                    + cleanup_semantic_score(&edit, &equality2);
                if score >= best_score {
                    best_score = score;
                    best_eq1 = equality1.clone();
                    best_edit = edit.clone();
                    best_eq2 = equality2.clone();
                }
            }

            if diffs[pointer - 1].text() != best_eq1.as_slice() {
                if !best_eq1.is_empty() {
                    diffs[pointer - 1] = DiffOp::Equal(best_eq1);
                } else {
                    diffs.remove(pointer - 1);
                    pointer -= 1;
                }
                diffs[pointer] = diffs[pointer].with_text(best_edit);
                if !best_eq2.is_empty() {
                    diffs[pointer + 1] = DiffOp::Equal(best_eq2);
                } else {
                    diffs.remove(pointer + 1);
                    pointer -= 1;
                }
            }
        }
        pointer += 1;
    }
}

fn cleanup_semantic_score(one: &[u16], two: &[u16]) -> usize {
    if one.is_empty() || two.is_empty() {
        return 6;
    }
    let char1 = one[one.len() - 1];
    let char2 = two[0];
    let non_alpha1 = !is_alphanumeric(char1);
    let non_alpha2 = !is_alphanumeric(char2);
    let whitespace1 = non_alpha1 && is_whitespace(char1);
    let whitespace2 = non_alpha2 && is_whitespace(char2);
    let linebreak1 = whitespace1 && is_linebreak(char1);
    let linebreak2 = whitespace2 && is_linebreak(char2);
    let blank_line1 = linebreak1 && is_blank_line_end(one);
    let blank_line2 = linebreak2 && is_blank_line_start(two);

    if blank_line1 || blank_line2 {
        5
    } else if linebreak1 || linebreak2 {
        4
    } else if non_alpha1 && !whitespace1 && whitespace2 {
        3
    } else if whitespace1 || whitespace2 {
        2
    } else if non_alpha1 || non_alpha2 {
        1
    } else {
        0
    }
}

/// `/[a-zA-Z0-9]/`: diff-match-patch's alphanumerics are ASCII only.
fn is_alphanumeric(unit: u16) -> bool {
    u8::try_from(unit).is_ok_and(|c| c.is_ascii_alphanumeric())
}

/// JavaScript's `/\s/`: Unicode White_Space without U+0085, plus U+FEFF.
fn is_whitespace(unit: u16) -> bool {
    char::from_u32(unit.into())
        .is_some_and(|c| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}')
}

fn is_linebreak(unit: u16) -> bool {
    unit == CR || unit == LF
}

const CR: u16 = b'\r' as u16;
const LF: u16 = b'\n' as u16;

/// `/\n\r?\n$/`
fn is_blank_line_end(text: &[u16]) -> bool {
    matches!(text, [.., LF, LF] | [.., LF, CR, LF])
}

/// `/^\r?\n\r?\n/`
fn is_blank_line_start(text: &[u16]) -> bool {
    matches!(
        text,
        [LF, LF, ..] | [LF, CR, LF, ..] | [CR, LF, LF, ..] | [CR, LF, CR, LF, ..]
    )
}

/// The length of the longest suffix of `text1` that is a prefix of `text2`.
fn overlap(text1: &[u16], text2: &[u16]) -> usize {
    (1..=text1.len().min(text2.len()))
        .rev()
        .find(|&length| text1[text1.len() - length..] == text2[..length])
        .unwrap_or(0)
}

/// diff-match-patch's `xIndex`: where `loc` in text1 lands in text2.
pub fn x_index(diffs: &[DiffOp], loc: usize) -> usize {
    let mut chars1: usize = 0;
    let mut chars2: usize = 0;
    let mut last_chars1: usize = 0;
    let mut last_chars2: usize = 0;
    let mut found_idx = None;

    for (x, diff) in diffs.iter().enumerate() {
        if !matches!(diff, DiffOp::Insert(_)) {
            chars1 += diff.text().len();
        }
        if !matches!(diff, DiffOp::Delete(_)) {
            chars2 += diff.text().len();
        }
        if chars1 > loc {
            found_idx = Some(x);
            break;
        }
        last_chars1 = chars1;
        last_chars2 = chars2;
    }

    if let Some(found) = found_idx
        && matches!(diffs[found], DiffOp::Delete(_))
    {
        return last_chars2;
    }
    last_chars2 + (loc - last_chars1)
}

/// diff-match-patch's `match`: the best place for `pattern` (at most 32 units, as in JavaScript)
/// near `loc`, within `threshold`.
pub fn match_bitap(
    text: &[u16],
    pattern: &[u16],
    loc: usize,
    threshold: f64,
    distance: usize,
) -> Option<usize> {
    let loc = loc.min(text.len());
    if text == pattern {
        return Some(0);
    }
    if text.is_empty() {
        return None;
    }
    if text[loc..(loc + pattern.len()).min(text.len())] == *pattern {
        return Some(loc);
    }
    bitap(text, pattern, loc, threshold, distance)
}

/// JavaScript's `haystack.lastIndexOf(needle, from)` for a non-empty `needle`.
fn rfind(haystack: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    let end = from.min(haystack.len().checked_sub(needle.len())?) + needle.len();
    haystack[..end]
        .windows(needle.len())
        .rposition(|w| w == needle)
}

/// diff-match-patch's `bitap`. Offsets are signed, as JavaScript's numbers are: the scan's start
/// moves to `2 * loc - best`, which is negative when a match is more than twice as far as `loc`.
fn bitap(
    text: &[u16],
    pattern: &[u16],
    loc: usize,
    threshold: f64,
    distance: usize,
) -> Option<usize> {
    let loc = loc as i64;
    let score = |errors: usize, x: i64| {
        let accuracy = errors as f64 / pattern.len() as f64;
        let proximity = (loc - x).abs() as f64;
        if distance == 0 {
            if proximity != 0.0 { 1.0 } else { accuracy }
        } else {
            accuracy + proximity / distance as f64
        }
    };

    let mut alphabet: HashMap<u16, u32> = HashMap::new();
    for (i, &c) in pattern.iter().enumerate() {
        *alphabet.entry(c).or_insert(0) |= 1 << (pattern.len() - 1 - i);
    }

    // A nearby exact match (either way) tightens the threshold.
    let mut score_threshold = threshold;
    if let Some(at) = index_of(text, pattern, loc as usize) {
        score_threshold = score_threshold.min(score(0, at as i64));
        if let Some(at) = rfind(text, pattern, loc as usize + pattern.len()) {
            score_threshold = score_threshold.min(score(0, at as i64));
        }
    }

    let matchmask = 1u32 << (pattern.len() - 1);
    let mut best_loc: i64 = -1;
    let mut bin_max = (pattern.len() + text.len()) as i64;
    let mut last_rd: Vec<u32> = Vec::new();
    for d in 0..pattern.len() {
        // How far from `loc` this many errors may stray.
        let mut bin_min = 0;
        let mut bin_mid = bin_max;
        while bin_min < bin_mid {
            if score(d, loc + bin_mid) <= score_threshold {
                bin_min = bin_mid;
            } else {
                bin_max = bin_mid;
            }
            bin_mid = (bin_max - bin_min) / 2 + bin_min;
        }
        bin_max = bin_mid;
        let mut start = (loc - bin_mid + 1).max(1);
        let finish = (loc + bin_mid).min(text.len() as i64) + pattern.len() as i64;

        let mut rd = vec![0u32; finish as usize + 2];
        rd[finish as usize + 1] = (1u32 << d) - 1;
        let last = |j: usize| last_rd.get(j).copied().unwrap_or(0);
        let mut j = finish;
        while j >= start {
            let ju = j as usize;
            let char_match = text
                .get(ju - 1)
                .and_then(|c| alphabet.get(c))
                .copied()
                .unwrap_or(0);
            rd[ju] = ((rd[ju + 1] << 1) | 1) & char_match;
            if d > 0 {
                rd[ju] |= (((last(ju + 1) | last(ju)) << 1) | 1) | last(ju + 1);
            }
            if rd[ju] & matchmask != 0 {
                let match_score = score(d, j - 1);
                if match_score <= score_threshold {
                    score_threshold = match_score;
                    best_loc = j - 1;
                    if best_loc > loc {
                        // Past `loc`: don't stray further from it than this match.
                        start = (2 * loc - best_loc).max(1);
                    } else {
                        break;
                    }
                }
            }
            j -= 1;
        }
        // No hope for a better match with more errors.
        if score(d + 1, loc) > score_threshold {
            break;
        }
        last_rd = rd;
    }
    usize::try_from(best_loc).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_u16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn x_index_basic() {
        let diffs = vec![
            DiffOp::Equal(to_u16("ab")),
            DiffOp::Delete(to_u16("c")),
            DiffOp::Insert(to_u16("xyz")),
            DiffOp::Equal(to_u16("de")),
        ];
        assert_eq!(x_index(&diffs, 0), 0);
        assert_eq!(x_index(&diffs, 2), 2);
        // "d", after the deleted "c", follows the inserted "xyz".
        assert_eq!(x_index(&diffs, 3), 5);
    }

    #[test]
    fn cleanup_absorbs_short_equality() {
        let needle = to_u16("const inflight = new Set();");
        let region = to_u16(" gone. */\n    const inflight = new Map();\n    /** Reque");
        let mut diffs = make_diff(&needle, &region, false, Duration::from_secs(1));
        cleanup_semantic(&mut diffs);
        assert!(
            !diffs
                .iter()
                .any(|d| d.is_equal() && d.text() == to_u16("();")),
            "Equal '();' should have been absorbed"
        );
    }

    /// Replays fixtures/dmp.json, recorded from @sanity/diff-match-patch by tests/gen/gen.mjs.
    #[test]
    fn matches_javascript_diff_match_patch() {
        let cases: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../tests/fixtures/dmp.json")).expect("dmp.json");
        let mut failures = Vec::new();
        for case in &cases {
            let (name, input, expected) = (&case["name"], &case["input"], &case["expected"]);
            let text = |key: &str| to_u16(input[key].as_str().expect("text input"));
            match input["type"].as_str() {
                Some("match") => {
                    let at = std::panic::catch_unwind(|| {
                        match_bitap(
                            &text("text"),
                            &text("pattern"),
                            input["loc"].as_u64().expect("loc") as usize,
                            input["threshold"].as_f64().expect("threshold"),
                            input["distance"].as_u64().expect("distance") as usize,
                        )
                    });
                    let want = expected["at"].as_u64().map(|at| at as usize);
                    if at.as_ref().ok() != Some(&want) {
                        failures.push(format!("{name}: expected {want:?}, got {at:?}"));
                    }
                }
                Some("cleanup") => {
                    let mut diffs =
                        make_diff(&text("a"), &text("b"), false, Duration::from_secs(1));
                    cleanup_semantic(&mut diffs);
                    let got: Vec<(i64, String)> = diffs
                        .iter()
                        .map(|d| {
                            let op = match d {
                                DiffOp::Delete(_) => -1,
                                DiffOp::Equal(_) => 0,
                                DiffOp::Insert(_) => 1,
                            };
                            (op, String::from_utf16_lossy(d.text()))
                        })
                        .collect();
                    let want: Vec<(i64, String)> =
                        serde_json::from_value(expected["diffs"].clone()).expect("diffs");
                    if got != want {
                        failures.push(format!("{name}: expected {want:?}, got {got:?}"));
                    }
                }
                // Replayed by review_anchor's tests.
                Some("track") => {}
                other => failures.push(format!("{name}: unknown case type {other:?}")),
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} cases failed:\n{}",
            failures.len(),
            cases.len(),
            failures.join("\n")
        );
    }
}
