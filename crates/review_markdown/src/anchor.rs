//! Text anchors on a rendered page (`text-anchor.ts`): a quote with 40 characters of context on each side and where
//! it was selected, and the place on the page such an anchor marks.

use review_model::{DocAnchor, DocAnchorType};

use crate::text::is_js_space;

/// Characters of context kept on each side of a quote.
const CONTEXT: usize = 40;

/// The anchor of a selection from `start` to `end` (UTF-16 offsets) of the page's `text`, without the whitespace at
/// either end of the selection; `None` when less than 3 characters remain.
pub fn text_anchor(text: &str, start: usize, end: usize) -> Option<DocAnchor> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let (mut start, mut end) = (start.min(units.len()), end.min(units.len()));
    if end <= start {
        return None;
    }
    while start < end && units.get(start).copied().is_some_and(is_js_space) {
        start += 1;
    }
    while end > start && units.get(end - 1).copied().is_some_and(is_js_space) {
        end -= 1;
    }
    if end - start < 3 {
        return None;
    }
    let slice = |from: usize, to: usize| String::from_utf16_lossy(units.get(from..to).unwrap_or_default());
    Some(DocAnchor {
        anchor_type: DocAnchorType::Text,
        exact: Some(slice(start, end)),
        prefix: Some(slice(start.saturating_sub(CONTEXT), start)),
        suffix: Some(slice(end, (end + CONTEXT).min(units.len()))),
        start_offset_hint: i64::try_from(start).ok(),
        end_offset_hint: i64::try_from(end).ok(),
        block_id: None,
        node_id: None,
        edge_id: None,
        source: None,
        occurrence: None,
        extra: Default::default(),
    })
}

/// Where a text anchor is on the page (`markText`): the UTF-16 range of the occurrence its tracked `occurrence`
/// names, else the one whose prefix and suffix match, the selected place breaking ties.
pub fn locate(text: &str, anchor: &DocAnchor) -> Option<(usize, usize)> {
    let exact: Vec<u16> = anchor.exact.as_deref().unwrap_or("").encode_utf16().collect();
    if exact.is_empty() {
        return None;
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let candidates: Vec<usize> = units
        .windows(exact.len())
        .enumerate()
        .filter(|(_, window)| *window == exact.as_slice())
        .map(|(at, _)| at)
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let span = |at: usize| (at, at + exact.len());
    if let Some(&at) = anchor
        .occurrence
        .and_then(|occurrence| candidates.get(occurrence as usize))
    {
        return Some(span(at));
    }
    let prefix: Vec<u16> = anchor.prefix.as_deref().unwrap_or("").encode_utf16().collect();
    let suffix: Vec<u16> = anchor.suffix.as_deref().unwrap_or("").encode_utf16().collect();
    let hint = anchor.start_offset_hint;
    let score = |at: usize| {
        let before = !prefix.is_empty()
            && at >= prefix.len()
            && units.get(at - prefix.len()..at) == Some(prefix.as_slice());
        let end = at + exact.len();
        let after = !suffix.is_empty()
            && units.get(end..(end + suffix.len()).min(units.len())) == Some(suffix.as_slice());
        let distance = hint.map_or(0.0, |hint| (at as f64 - hint as f64).abs());
        f64::from(u8::from(before) * 2 + u8::from(after) * 2) - distance / 1e6
    };
    // Stable, as JavaScript's sort: equal scores keep document order.
    let mut best = candidates.first().copied()?;
    let mut best_score = score(best);
    for &at in candidates.iter().skip(1) {
        let candidate = score(at);
        if candidate > best_score {
            best = at;
            best_score = candidate;
        }
    }
    Some(span(best))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_drops_its_surrounding_whitespace() {
        let anchor = text_anchor("one two three", 3, 8).expect("an anchor");
        assert_eq!(anchor.exact.as_deref(), Some("two"));
        assert_eq!(anchor.prefix.as_deref(), Some("one "));
        assert_eq!(anchor.suffix.as_deref(), Some(" three"));
        assert_eq!(anchor.start_offset_hint, Some(4));
        assert_eq!(text_anchor("one two", 3, 5), None);
    }

    #[test]
    fn locate_prefers_context_then_the_hint() {
        let text = "the size is set. the size defaults.";
        let mut anchor = text_anchor(text, 17, 25).expect("an anchor");
        assert_eq!(locate(text, &anchor), Some((17, 25)));
        anchor.prefix = None;
        anchor.suffix = None;
        assert_eq!(locate(text, &anchor), Some((17, 25)));
        anchor.start_offset_hint = None;
        assert_eq!(locate(text, &anchor), Some((0, 8)));
        anchor.occurrence = Some(1);
        assert_eq!(locate(text, &anchor), Some((17, 25)));
    }
}
