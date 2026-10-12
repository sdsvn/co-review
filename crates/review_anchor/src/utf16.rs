//! UTF-16 text operations for compatibility with JavaScript string semantics.
//!
//! JavaScript strings are sequences of UTF-16 code units. All offsets in TrackedRange and Edit
//! are UTF-16 code unit offsets, so the algorithm must operate on those.

/// A text held as UTF-16 code units for offset compatibility with JavaScript.
#[derive(Clone, Debug)]
pub struct Utf16Text {
    utf16: Vec<u16>,
}

impl Utf16Text {
    pub fn new(s: &str) -> Self {
        Self {
            utf16: s.encode_utf16().collect(),
        }
    }

    pub fn utf16_units(&self) -> &[u16] {
        &self.utf16
    }

    pub fn len_utf16(&self) -> usize {
        self.utf16.len()
    }

    /// Slice by UTF-16 code unit offsets, returning the substring as a new Utf16Text.
    pub fn slice(&self, from: usize, to: usize) -> Self {
        let from = from.min(self.utf16.len());
        let to = to.min(self.utf16.len()).max(from);
        Self {
            utf16: self.utf16[from..to].to_vec(),
        }
    }

    /// Slice returning just the String (avoids double allocation when Utf16Text isn't needed).
    pub fn slice_str(&self, from: usize, to: usize) -> String {
        let from = from.min(self.utf16.len());
        let to = to.min(self.utf16.len()).max(from);
        String::from_utf16_lossy(&self.utf16[from..to])
    }

    /// Find the first occurrence of `needle` (UTF-16 units) starting at UTF-16 offset `start`.
    /// Returns the UTF-16 offset, or None.
    pub fn find_utf16(&self, needle: &[u16], start: usize) -> Option<usize> {
        if needle.is_empty() {
            return Some(start.min(self.utf16.len()));
        }
        if start + needle.len() > self.utf16.len() {
            return None;
        }
        for i in start..=self.utf16.len() - needle.len() {
            if self.utf16[i..i + needle.len()] == *needle {
                return Some(i);
            }
        }
        None
    }

    /// Index of '\n' in the UTF-16 units starting at `start`. Returns UTF-16 offset.
    pub fn find_newline(&self, start: usize) -> Option<usize> {
        let newline = b'\n' as u16;
        self.utf16[start..].iter().position(|&u| u == newline).map(|p| start + p)
    }

    /// Last index of '\n' searching backwards from `before` (exclusive). Returns UTF-16 offset.
    pub fn rfind_newline(&self, before: usize) -> Option<usize> {
        let newline = b'\n' as u16;
        let end = before.min(self.utf16.len());
        self.utf16[..end].iter().rposition(|&u| u == newline)
    }

    /// Get a single UTF-16 code unit, if in bounds.
    pub fn unit_at(&self, index: usize) -> Option<u16> {
        self.utf16.get(index).copied()
    }
}

/// Whether a UTF-16 code unit is a word character (\p{L}, \p{N}, or '_').
/// For surrogate halves, returns false (matching JS behavior).
pub fn is_word_char(unit: u16) -> bool {
    if unit == b'_' as u16 {
        return true;
    }
    if (0xD800..=0xDFFF).contains(&unit) {
        return false;
    }
    match char::from_u32(unit as u32) {
        Some(c) => c.is_alphanumeric(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_ascii() {
        let t = Utf16Text::new("hello world");
        assert_eq!(t.len_utf16(), 11);
        assert_eq!(t.slice_str(0, 5), "hello");
    }

    #[test]
    fn astral_characters() {
        let t = Utf16Text::new("a😀b");
        assert_eq!(t.len_utf16(), 4);
        assert_eq!(t.slice_str(0, 1), "a");
        assert_eq!(t.slice_str(1, 3), "😀");
        assert_eq!(t.slice_str(3, 4), "b");
    }

    #[test]
    fn word_char_check() {
        assert!(is_word_char(b'a' as u16));
        assert!(is_word_char(b'Z' as u16));
        assert!(is_word_char(b'0' as u16));
        assert!(is_word_char(b'_' as u16));
        assert!(!is_word_char(b' ' as u16));
        assert!(!is_word_char(b'(' as u16));
        assert!(!is_word_char(0xD83D));
    }
}
