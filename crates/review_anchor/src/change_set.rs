//! A minimal reimplementation of CodeMirror's ChangeSet, covering only the operations
//! used by anchor tracking: `map_pos` (with assoc) and `iter_changed_ranges`.
//!
//! All positions are UTF-16 code unit offsets.

use crate::Edit;

/// A set of non-overlapping, sorted edits applied to a document of known length.
pub struct ChangeSet {
    /// Sorted, non-overlapping edits in the old document.
    edits: Vec<Edit>,
}

impl ChangeSet {
    /// Build a ChangeSet from edits and the old document's UTF-16 length.
    /// Edits must be sorted by `from` and non-overlapping.
    pub fn from_edits(edits: Vec<Edit>, _old_length: usize) -> Self {
        Self { edits }
    }

    /// Map a position in the old document to the new document.
    ///
    /// `assoc` controls behavior at edit boundaries:
    /// - `assoc > 0`: an insertion at exactly `pos` pushes it to the right (after the insertion)
    /// - `assoc < 0`: an insertion at exactly `pos` keeps it at the left (before the insertion)
    /// - `assoc == 0`: treated as `assoc > 0`
    pub fn map_pos(&self, pos: usize, assoc: i32) -> usize {
        let mut shift: isize = 0;
        for edit in &self.edits {
            if edit.from > pos || (edit.from == pos && assoc < 0 && edit.from == edit.to) {
                // Edit is entirely after `pos`, or is a pure insertion at pos with assoc < 0
                break;
            }
            if edit.from == pos && edit.from == edit.to {
                // Pure insertion at pos with assoc >= 0: push past it
                shift += edit.insert_len() as isize;
                continue;
            }
            if edit.to <= pos {
                // Edit is entirely before pos
                shift += edit.insert_len() as isize - (edit.to as isize - edit.from as isize);
            } else if edit.from <= pos {
                // Pos is inside the edit range
                if assoc < 0 {
                    // Map to the start of the replacement
                    return (edit.from as isize + shift) as usize;
                } else {
                    // Map to the end of the replacement
                    return (edit.from as isize + shift + edit.insert_len() as isize) as usize;
                }
            }
        }
        (pos as isize + shift) as usize
    }

    /// Iterate over changed ranges, calling the callback with
    /// `(from_a, to_a, from_b, to_b)` where (from_a, to_a) is the range in the old
    /// document and (from_b, to_b) is the corresponding range in the new document.
    pub fn iter_changed_ranges<F: FnMut(usize, usize, usize, usize)>(&self, mut callback: F) {
        let mut shift: isize = 0;
        for edit in &self.edits {
            let from_b = (edit.from as isize + shift) as usize;
            let to_b = from_b + edit.insert_len();
            callback(edit.from, edit.to, from_b, to_b);
            shift += edit.insert_len() as isize - (edit.to as isize - edit.from as isize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_pos_insertion_at_pos() {
        // Insert "abc" (3 chars) at position 5
        let cs = ChangeSet::from_edits(
            vec![Edit { from: 5, to: 5, insert: "abc".to_string() }],
            10,
        );
        // assoc=1: insertion at pos pushes it right
        assert_eq!(cs.map_pos(5, 1), 8);
        // assoc=-1: insertion at pos keeps it left
        assert_eq!(cs.map_pos(5, -1), 5);
        // Before the insertion: unchanged
        assert_eq!(cs.map_pos(3, 1), 3);
        // After the insertion: shifted by 3
        assert_eq!(cs.map_pos(7, 1), 10);
    }

    #[test]
    fn map_pos_replacement() {
        // Replace positions 3-6 with "xy" (2 chars replacing 3)
        let cs = ChangeSet::from_edits(
            vec![Edit { from: 3, to: 6, insert: "xy".to_string() }],
            10,
        );
        // Before: unchanged
        assert_eq!(cs.map_pos(2, 1), 2);
        // At start of replacement with assoc=-1: maps to start
        assert_eq!(cs.map_pos(3, -1), 3);
        // Inside replacement with assoc=1: maps to end
        assert_eq!(cs.map_pos(4, 1), 5);
        // After: shifted by -1
        assert_eq!(cs.map_pos(7, 1), 6);
    }

    #[test]
    fn iter_changed_ranges_basic() {
        let cs = ChangeSet::from_edits(
            vec![
                Edit { from: 2, to: 4, insert: "xyz".to_string() },
                Edit { from: 8, to: 8, insert: "!".to_string() },
            ],
            12,
        );
        let mut ranges = vec![];
        cs.iter_changed_ranges(|fa, ta, fb, tb| ranges.push((fa, ta, fb, tb)));
        assert_eq!(ranges, vec![(2, 4, 2, 5), (8, 8, 9, 10)]);
    }
}
