//! UTF-16 offsets (JavaScript's) and byte offsets (Rust's) into the same text.

/// The length of `text` in UTF-16 code units.
pub fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// The byte offset in `text` of the UTF-16 offset `units`, rounded down to a character boundary; the end of the text
/// when it is past it.
pub fn utf16_to_byte(text: &str, units: usize) -> usize {
    let mut seen = 0;
    for (byte, character) in text.char_indices() {
        let next = seen + character.len_utf16();
        if next > units {
            return byte;
        }
        seen = next;
    }
    text.len()
}

/// The UTF-16 offset of the byte offset `byte` in `text` (rounded down to a character boundary).
pub fn byte_to_utf16(text: &str, byte: usize) -> usize {
    text.char_indices()
        .take_while(|(index, _)| *index < byte)
        .map(|(_, character)| character.len_utf16())
        .sum()
}

/// JavaScript's `/\s/` on one UTF-16 code unit.
pub(crate) fn is_js_space(unit: u16) -> bool {
    unit == 0xfeff || char::from_u32(u32::from(unit)).is_some_and(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_round_trip_through_astral_characters() {
        let text = "a😀b✎c";
        assert_eq!(utf16_len(text), 6);
        assert_eq!(utf16_to_byte(text, 3), 5);
        assert_eq!(byte_to_utf16(text, 5), 3);
        // Inside the surrogate pair: the character's start.
        assert_eq!(utf16_to_byte(text, 2), 1);
        assert_eq!(utf16_to_byte(text, 99), text.len());
    }
}
