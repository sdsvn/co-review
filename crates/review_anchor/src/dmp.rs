/// Diff-match-patch operations on UTF-16 code unit slices.
///
/// A faithful port of the parts of @sanity/diff-match-patch used by anchor-tracking:
/// `makeDiff` (with checkLines mode), `cleanupSemantic`, `match` (bitap fuzzy), and `xIndex`.
///
/// All operations work on &[u16] to match JavaScript's UTF-16 string semantics exactly.
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A diff operation: Equal, Insert, or Delete, with a slice of UTF-16 code units.
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
pub fn make_diff(text1: &[u16], text2: &[u16], check_lines: bool, timeout: Duration) -> Vec<DiffOp> {
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

    // Trim common prefix
    let common_prefix_len = common_prefix(text1, text2);
    let prefix = text1[..common_prefix_len].to_vec();
    let text1 = &text1[common_prefix_len..];
    let text2 = &text2[common_prefix_len..];

    // Trim common suffix
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

    // Re-attach prefix and suffix
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

    // Convert back from line chars to actual text
    for diff in &mut diffs {
        let chars = diff.text();
        let mut text = Vec::new();
        for &c in chars {
            let idx = c as usize;
            if idx < line_array.len() {
                text.extend_from_slice(&line_array[idx]);
            }
        }
        *diff = diff.with_text(text);
    }

    // Character-level refinement within non-equal runs
    merge_diffs(&mut diffs);
    let mut result = Vec::new();
    let mut i = 0;
    while i < diffs.len() {
        if diffs[i].is_equal() {
            result.push(diffs[i].clone());
            i += 1;
            continue;
        }
        // Collect the non-equal run
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
        // Refine this hunk at character level
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
    let mut line_array: Vec<Vec<u16>> = vec![vec![]]; // Index 0 unused
    let mut line_hash: HashMap<Vec<u16>, u16> = HashMap::new();

    let encode = |text: &[u16], line_array: &mut Vec<Vec<u16>>, line_hash: &mut HashMap<Vec<u16>, u16>| -> Vec<u16> {
        let mut chars = Vec::new();
        let mut start = 0;
        while start < text.len() {
            let end = text[start..].iter().position(|&c| c == b'\n' as u16)
                .map(|p| start + p + 1)
                .unwrap_or(text.len());
            let line = text[start..end].to_vec();
            if let Some(&idx) = line_hash.get(&line) {
                chars.push(idx);
            } else {
                let idx = line_array.len() as u16;
                line_array.push(line.clone());
                line_hash.insert(line, idx);
                chars.push(idx);
            }
            start = end;
        }
        chars
    };

    let chars1 = encode(text1, &mut line_array, &mut line_hash);
    let chars2 = encode(text2, &mut line_array, &mut line_hash);
    (chars1, chars2, line_array)
}

/// The core diff algorithm (Myers' diff).
fn diff_compute(text1: &[u16], text2: &[u16], deadline: Instant) -> Vec<DiffOp> {
    if text1.is_empty() {
        return vec![DiffOp::Insert(text2.to_vec())];
    }
    if text2.is_empty() {
        return vec![DiffOp::Delete(text1.to_vec())];
    }

    // Check for substring
    if let Some(pos) = find_substr(text1, text2) {
        // text2 is a substring of text1... no, other way:
        // actually DMP checks if one is substring of other
        // Let me handle the case properly
        // If text2 contains text1
        return vec![
            DiffOp::Delete(text1[..pos].to_vec()),
            DiffOp::Equal(text2.to_vec()),
            DiffOp::Delete(text1[pos + text2.len()..].to_vec()),
        ].into_iter().filter(|d| !d.text().is_empty()).collect();
    }

    if let Some(pos) = find_substr(text2, text1) {
        return vec![
            DiffOp::Insert(text2[..pos].to_vec()),
            DiffOp::Equal(text1.to_vec()),
            DiffOp::Insert(text2[pos + text1.len()..].to_vec()),
        ].into_iter().filter(|d| !d.text().is_empty()).collect();
    }

    // Check for single character: if text1 is a single unit
    if text1.len() == 1 {
        return vec![DiffOp::Delete(text1.to_vec()), DiffOp::Insert(text2.to_vec())];
    }
    if text2.len() == 1 {
        return vec![DiffOp::Delete(text1.to_vec()), DiffOp::Insert(text2.to_vec())];
    }

    // Try half-match (long texts only)
    let hm = half_match(text1, text2);
    if let Some((prefix1, suffix1, prefix2, suffix2, common)) = hm {
        let diffs_a = diff_compute(&prefix1, &prefix2, deadline);
        let diffs_b = diff_compute(&suffix1, &suffix2, deadline);
        let mut result = diffs_a;
        result.push(DiffOp::Equal(common));
        result.extend(diffs_b);
        return result;
    }

    diff_bisect(text1, text2, deadline)
}

/// Find `needle` as a substring of `haystack`. Returns offset in haystack.
fn find_substr(haystack: &[u16], needle: &[u16]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    for i in 0..=haystack.len() - needle.len() {
        if haystack[i..i + needle.len()] == *needle {
            return Some(i);
        }
    }
    None
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
                if k2_offset >= 0 && (k2_offset as usize) < v2.len() && v2[k2_offset as usize] != -1 {
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
            while x1 < text1_len && y1 < text2_len
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
                if k2_offset >= 0 && (k2_offset as usize) < v1.len() && v1[k2_offset as usize] != -1 {
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
    vec![DiffOp::Delete(text1.to_vec()), DiffOp::Insert(text2.to_vec())]
}

fn diff_bisect_split(text1: &[u16], text2: &[u16], x: usize, y: usize, deadline: Instant) -> Vec<DiffOp> {
    let mut diffs = diff_compute(&text1[..x], &text2[..y], deadline);
    diffs.extend(diff_compute(&text1[x..], &text2[y..], deadline));
    diffs
}

/// Five-tuple returned by `half_match`: (text1_prefix, text1_suffix, text2_prefix, text2_suffix, common_middle).
type HalfMatch = (Vec<u16>, Vec<u16>, Vec<u16>, Vec<u16>, Vec<u16>);

/// Find a shared substring that is at least half the length of the longer text.
fn half_match(text1: &[u16], text2: &[u16]) -> Option<HalfMatch> {
    let longtext = if text1.len() > text2.len() { text1 } else { text2 };
    let shorttext = if text1.len() > text2.len() { text2 } else { text1 };
    if longtext.len() < 4 || shorttext.len() * 2 < longtext.len() {
        return None;
    }

    let hm1 = half_match_inner(longtext, shorttext, longtext.len().div_ceil(4));
    let hm2 = half_match_inner(longtext, shorttext, longtext.len().div_ceil(2));
    let hm = match (hm1, hm2) {
        (None, None) => return None,
        (Some(h), None) | (None, Some(h)) => h,
        (Some(h1), Some(h2)) => {
            if h1.4.len() > h2.4.len() { h1 } else { h2 }
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
    let mut j_opt = find_substr(shorttext, seed);
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
        j_opt = if j + 1 < shorttext.len() {
            find_substr(&shorttext[j + 1..], seed).map(|p| p + j + 1)
        } else {
            None
        };
    }
    if best.4.len() * 2 >= longtext.len() {
        Some(best)
    } else {
        None
    }
}

fn common_prefix(text1: &[u16], text2: &[u16]) -> usize {
    let n = text1.len().min(text2.len());
    for i in 0..n {
        if text1[i] != text2[i] {
            return i;
        }
    }
    n
}

fn common_suffix(text1: &[u16], text2: &[u16]) -> usize {
    let n = text1.len().min(text2.len());
    for i in 1..=n {
        if text1[text1.len() - i] != text2[text2.len() - i] {
            return i - 1;
        }
    }
    n
}

/// DMP cleanupMerge: collects consecutive deletes and inserts into canonical form
/// (all deletes before all inserts in each run), factoring out common prefix/suffix
/// between the delete and insert text.
fn merge_diffs(diffs: &mut Vec<DiffOp>) {
    diffs.push(DiffOp::Equal(vec![])); // sentinel
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
                        // Factor out common prefix
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
                        // Factor out common suffix
                        let cs = common_suffix(&text_insert, &text_delete);
                        if cs > 0 {
                            let suffix = text_insert[text_insert.len() - cs..].to_vec();
                            let mut eq = suffix;
                            eq.extend_from_slice(diffs[pointer].text());
                            diffs[pointer] = DiffOp::Equal(eq);
                            text_insert = text_insert[..text_insert.len() - cs].to_vec();
                            text_delete = text_delete[..text_delete.len() - cs].to_vec();
                        }
                    }
                    // Remove the old entries
                    let base = pointer - count_delete - count_insert;
                    let remove_count = count_delete + count_insert;
                    diffs.drain(base..base + remove_count);
                    pointer = base;
                    // Insert the canonical form: deletes first, then inserts
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
                    // Merge consecutive equalities
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
    // Remove sentinel if still empty
    if diffs.last().is_some_and(|d| d.text().is_empty()) {
        diffs.pop();
    }

    // Second pass: handle sliding patterns where an edit is bracketed by equalities
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
                // edit ends with prev_text: slide left
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
                // edit starts with next_text: slide right
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
        let cloned = diffs.clone();
        diffs.clear();
        diffs.extend(cloned);
        merge_diffs(diffs);
    }
}

/// Cleanup semantic: make the diffs read like edits by sliding diff boundaries to word/line breaks.
/// Port of diff-match-patch's cleanupSemantic.
pub fn cleanup_semantic(diffs: &mut Vec<DiffOp>) {
    if diffs.is_empty() {
        return;
    }

    let mut changes = true;
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
                    // Splice the equality into the surrounding edits
                    let insert_at = equalities[equalities.len() - 1];
                    diffs.insert(insert_at, DiffOp::Delete(eq.clone()));
                    // The insert/equal after it becomes an insert
                    diffs[insert_at + 1] = DiffOp::Insert(diffs[insert_at + 1].text().to_vec());
                    // Remove this equality from the stack
                    equalities.pop();
                    if !equalities.is_empty() {
                        equalities.pop();
                    }
                    pointer = if equalities.is_empty() { 0 } else { equalities[equalities.len() - 1] };
                    length_insertions1 = 0;
                    length_deletions1 = 0;
                    length_insertions2 = 0;
                    length_deletions2 = 0;
                    last_equality = None;
                    changes = true;

                    // Continue from this point (pointer might be 0 which means start over)
                    if pointer == 0 {
                        // Will be incremented below
                    }
                    // Skip the increment at the end since we want to re-examine this position
                    continue;
                }
        }
        pointer += 1;
    }

    if changes {
        merge_diffs(diffs);
    }

    cleanup_semantic_lossless(diffs);

    // Find overlaps between deletions and insertions.
    let mut pointer = 1;
    while pointer < diffs.len() {
        if pointer > 0 && matches!(diffs[pointer - 1], DiffOp::Delete(_)) && matches!(diffs[pointer], DiffOp::Insert(_)) {
            let deletion = diffs[pointer - 1].text().to_vec();
            let insertion = diffs[pointer].text().to_vec();
            let overlap_len1 = overlap(&deletion, &insertion);
            let overlap_len2 = overlap(&insertion, &deletion);
            if overlap_len1 >= overlap_len2 {
                if overlap_len1 as f64 >= deletion.len() as f64 / 2.0
                    || overlap_len1 as f64 >= insertion.len() as f64 / 2.0
                {
                    diffs.insert(pointer, DiffOp::Equal(insertion[..overlap_len1].to_vec()));
                    diffs[pointer - 1] = DiffOp::Delete(deletion[..deletion.len() - overlap_len1].to_vec());
                    diffs[pointer + 1] = DiffOp::Insert(insertion[overlap_len1..].to_vec());
                    pointer += 1;
                }
            } else if overlap_len2 as f64 >= deletion.len() as f64 / 2.0
                || overlap_len2 as f64 >= insertion.len() as f64 / 2.0
            {
                diffs.insert(pointer, DiffOp::Equal(deletion[..overlap_len2].to_vec()));
                diffs[pointer - 1] = DiffOp::Insert(insertion[..insertion.len() - overlap_len2].to_vec());
                diffs[pointer + 1] = DiffOp::Delete(deletion[overlap_len2..].to_vec());
                pointer += 1;
            }
            pointer += 1;
        }
        pointer += 1;
    }

    // Remove empty entries
    diffs.retain(|d| !d.text().is_empty());
}

fn cleanup_semantic_lossless(diffs: &mut Vec<DiffOp>) {
    let mut pointer = 1;
    while pointer + 1 < diffs.len() {
        if diffs[pointer - 1].is_equal() && diffs[pointer + 1].is_equal() {
            // Re-derive equality1, edit, equality2 from the diffs directly
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
    let non_alpha1 = !is_alpha_numeric(char1);
    let non_alpha2 = !is_alpha_numeric(char2);
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

fn is_alpha_numeric(c: u16) -> bool {
    if (0xD800..=0xDFFF).contains(&c) {
        return false;
    }
    match char::from_u32(c as u32) {
        Some(ch) => ch.is_alphanumeric(),
        None => false,
    }
}

fn is_whitespace(c: u16) -> bool {
    c == b' ' as u16 || c == b'\t' as u16 || c == b'\n' as u16 || c == b'\r' as u16
}

fn is_linebreak(c: u16) -> bool {
    c == b'\n' as u16 || c == b'\r' as u16
}

fn is_blank_line_end(text: &[u16]) -> bool {
    // Ends with two newlines (possibly with \r)
    let len = text.len();
    if len >= 2 && text[len - 1] == b'\n' as u16 && text[len - 2] == b'\n' as u16 {
        return true;
    }
    if len >= 3 && text[len - 1] == b'\n' as u16 && text[len - 2] == b'\r' as u16 && text[len - 3] == b'\n' as u16 {
        return true;
    }
    false
}

fn is_blank_line_start(text: &[u16]) -> bool {
    if text.len() >= 2 && text[0] == b'\n' as u16 && text[1] == b'\n' as u16 {
        return true;
    }
    if text.len() >= 2 && text[0] == b'\n' as u16 && text[1] == b'\r' as u16 {
        return true;
    }
    if text.len() >= 2 && text[0] == b'\r' as u16 && text[1] == b'\n' as u16 {
        return true;
    }
    false
}

/// Compute the length of the longest common overlap of the suffix of `text1` and prefix of `text2`.
fn overlap(text1: &[u16], text2: &[u16]) -> usize {
    let len1 = text1.len();
    let len2 = text2.len();
    if len1 == 0 || len2 == 0 {
        return 0;
    }
    let max = len1.min(len2);
    let mut best = 0;
    for length in 1..=max {
        if text1[len1 - length..] == text2[..length] {
            best = length;
        }
    }
    best
}

/// Map a position in text1 through the diffs to a position in text2.
/// Port of diff-match-patch's xIndex.
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

/// Fuzzy matching: wraps the DMP `match` function. First tries exact substring, then bitap.
/// Returns the offset of the best match, or None if no match within the threshold.
pub fn match_bitap(text: &[u16], pattern: &[u16], loc: usize, threshold: f64, distance: usize) -> Option<usize> {
    if pattern.is_empty() || text.is_empty() {
        return None;
    }

    let loc = loc.min(text.len());

    // Check for exact match at loc
    if loc + pattern.len() <= text.len() && text[loc..loc + pattern.len()] == *pattern {
        return Some(loc);
    }

    // Check for exact matches via substring search to set initial threshold
    let mut score_threshold = threshold;
    let mut best_loc: Option<isize> = None;

    if find_substr(text, pattern).is_some() {
        // indexOf(pattern, loc) in JS
        if let Some(idx) = find_substr_from(text, pattern, loc) {
            let s = bitap_score(pattern.len(), distance, 0, idx, loc);
            if s < score_threshold {
                score_threshold = s;
            }
            best_loc = Some(idx as isize);
        }

        // lastIndexOf from loc + pattern.length
        let last_search = (loc + pattern.len()).min(text.len());
        if let Some(idx) = rfind_substr(text, pattern, last_search) {
            let s = bitap_score(pattern.len(), distance, 0, idx, loc);
            if s < score_threshold {
                score_threshold = s;
            }
            best_loc = Some(idx as isize);
        }
    }

    bitap_core(text, pattern, loc, score_threshold, distance, best_loc)
}

fn find_substr_from(haystack: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from.min(haystack.len()));
    }
    if from + needle.len() > haystack.len() {
        return None;
    }
    for i in from..=haystack.len() - needle.len() {
        if haystack[i..i + needle.len()] == *needle {
            return Some(i);
        }
    }
    None
}

fn rfind_substr(haystack: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from.min(haystack.len()));
    }
    if needle.len() > haystack.len() {
        return None;
    }
    let start = from.min(haystack.len() - needle.len());
    for i in (0..=start).rev() {
        if haystack[i..i + needle.len()] == *needle {
            return Some(i);
        }
    }
    None
}

fn bitap_score(pattern_len: usize, distance: usize, errors: usize, x: usize, loc: usize) -> f64 {
    let accuracy = errors as f64 / pattern_len as f64;
    let proximity = (x as isize - loc as isize).unsigned_abs() as f64;
    if distance == 0 {
        if proximity != 0.0 { 1.0 } else { accuracy }
    } else {
        accuracy + proximity / distance as f64
    }
}

fn bitap_core(
    text: &[u16],
    pattern: &[u16],
    loc: usize,
    mut score_threshold: f64,
    distance: usize,
    initial_best: Option<isize>,
) -> Option<usize> {
    let pattern_len = pattern.len().min(32);

    // Initialize the alphabet
    let mut s: HashMap<u16, u32> = HashMap::new();
    for &c in pattern.iter().take(pattern_len) {
        s.entry(c).or_insert(0);
    }
    for (i, &c) in pattern.iter().take(pattern_len).enumerate() {
        let mask = s.entry(c).or_insert(0);
        *mask |= 1 << (pattern_len - 1 - i);
    }

    let matchmask = 1u32 << (pattern_len - 1);
    let mut best_loc: isize = initial_best.unwrap_or(-1);

    let mut bin_max = pattern_len + text.len();
    let mut last_rd: Vec<u32> = Vec::new();

    for d in 0..pattern_len {
        // Binary search for the window
        let mut bin_min: usize = 0;
        let mut bin_mid = bin_max;
        while bin_min < bin_mid {
            let s = bitap_score(pattern_len, distance, d, loc + bin_mid, loc);
            if s <= score_threshold {
                bin_min = bin_mid;
            } else {
                bin_max = bin_mid;
            }
            bin_mid = (bin_max - bin_min) / 2 + bin_min;
        }
        bin_max = bin_mid;

        let start = 1usize.max(loc.saturating_sub(bin_mid).max(1));
        let finish = (loc + bin_mid).min(text.len()) + pattern_len;

        let mut rd = vec![0u32; finish + 2];
        rd[finish + 1] = (1u32 << d) - 1;

        let mut j = finish;
        while j >= start {
            let char_match = if j > 0 && j <= text.len() {
                s.get(&text[j - 1]).copied().unwrap_or(0)
            } else {
                0
            };

            if d == 0 {
                rd[j] = ((rd[j + 1] << 1) | 1) & char_match;
            } else {
                let prev_j1 = if j + 1 < last_rd.len() { last_rd[j + 1] } else { 0 };
                let prev_j = if j < last_rd.len() { last_rd[j] } else { 0 };
                rd[j] = ((rd[j + 1] << 1) | 1) & char_match
                    | (((prev_j1 | prev_j) << 1) | 1)
                    | prev_j1;
            }

            if rd[j] & matchmask != 0 {
                let score = bitap_score(pattern_len, distance, d, j - 1, loc);
                if score <= score_threshold {
                    score_threshold = score;
                    best_loc = (j - 1) as isize;
                    if best_loc as usize > loc {
                        let new_start = 1usize.max(2 * loc - best_loc as usize);
                        if new_start > j {
                            break;
                        }
                        // start can only increase, not decrease, but the for loop handles it
                    } else {
                        break;
                    }
                }
            }

            if j == 0 {
                break;
            }
            j -= 1;
        }

        if bitap_score(pattern_len, distance, d + 1, loc, loc) > score_threshold {
            break;
        }

        last_rd = rd;
    }

    if best_loc >= 0 {
        Some(best_loc as usize)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_u16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn diff_equal_texts() {
        let text = to_u16("hello");
        let diffs = make_diff(&text, &text, false, Duration::from_secs(1));
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0].is_equal());
    }

    #[test]
    fn diff_insert() {
        let a = to_u16("abc");
        let b = to_u16("abcdef");
        let diffs = make_diff(&a, &b, false, Duration::from_secs(1));
        let mut result_a = Vec::new();
        let mut result_b = Vec::new();
        for d in &diffs {
            match d {
                DiffOp::Equal(t) | DiffOp::Delete(t) => result_a.extend_from_slice(t),
                _ => {}
            }
            match d {
                DiffOp::Equal(t) | DiffOp::Insert(t) => result_b.extend_from_slice(t),
                _ => {}
            }
        }
        assert_eq!(result_a, a);
        assert_eq!(result_b, b);
    }

    #[test]
    fn x_index_basic() {
        // Equal("ab"), Delete("c"), Insert("xyz"), Equal("de")
        let diffs = vec![
            DiffOp::Equal(to_u16("ab")),
            DiffOp::Delete(to_u16("c")),
            DiffOp::Insert(to_u16("xyz")),
            DiffOp::Equal(to_u16("de")),
        ];
        // Position 0 in text1 → 0 in text2
        assert_eq!(x_index(&diffs, 0), 0);
        // Position 1 → 1
        assert_eq!(x_index(&diffs, 1), 1);
        // Position 2 → 2 (still in the first equal)
        assert_eq!(x_index(&diffs, 2), 2);
        // Position 3 ("d" after "c" deleted) → 5 ("d" after "xyz")
        assert_eq!(x_index(&diffs, 3), 5);
    }

    #[test]
    fn match_exact() {
        let text = to_u16("abcdefghijklmnop");
        let pattern = to_u16("fgh");
        let result = match_bitap(&text, &pattern, 5, 0.5, 1000);
        assert_eq!(result, Some(5));
    }

    #[test]
    fn match_fuzzy() {
        let text = to_u16("abcdefghijklmnop");
        let pattern = to_u16("fgx");
        let result = match_bitap(&text, &pattern, 5, 0.5, 1000);
        assert!(result.is_some());
    }
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;

    fn to_u16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn cleanup_absorbs_short_equality() {
        // Raw diff: Insert(" gone. */\n    "), Equal("const inflight = new "), Delete("Set"), Insert("Map"), Equal("();"), Insert("\n    /** Reque")
        // After cleanup: Equal("const inflight = new ") should stay, but Equal("();") should be absorbed
        let needle = to_u16("const inflight = new Set();");
        let region = to_u16(" gone. */\n    const inflight = new Map();\n    /** Reque");
        let mut diffs = make_diff(&needle, &region, false, std::time::Duration::from_secs(1));
        cleanup_semantic(&mut diffs);

        // Check that "();" is not a separate Equal
        let eq_texts: Vec<String> = diffs.iter()
            .filter(|d| d.is_equal())
            .map(|d| String::from_utf16_lossy(d.text()))
            .collect();
        eprintln!("Diffs: {:?}", diffs.iter().map(|d| {
            let op = match d { DiffOp::Equal(_) => "=", DiffOp::Insert(_) => "+", DiffOp::Delete(_) => "-" };
            format!("{}\"{}\"", op, String::from_utf16_lossy(d.text()))
        }).collect::<Vec<_>>());
        
        // The expected cleaned diffs should be:
        // Insert(" gone. */\n    "), Equal("const inflight = new "), Delete("Set();"), Insert("Map();\n    /** Reque")
        // (i.e., "();" absorbed into the delete and insert)
        assert!(!eq_texts.iter().any(|t| t == "();"), "Equal '();' should have been absorbed");
    }
}
