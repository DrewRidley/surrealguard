//! Position/offset conversion utilities.

use tower_lsp::lsp_types::{Position, Range};

/// Convert a byte offset to an LSP Position (line, character).
pub fn offset_to_position(source: &str, offset: usize) -> Position {
    let target = offset.min(source.len());
    let mut line = 0u32;
    let mut character = 0u32;

    for (byte, ch) in source.char_indices() {
        if byte >= target {
            break;
        }
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += ch.len_utf16() as u32;
        }
    }

    Position::new(line, character)
}

/// Convert byte offsets to an LSP Range.
pub fn byte_range_to_lsp(source: &str, start: usize, end: usize) -> Range {
    Range {
        start: offset_to_position(source, start),
        end: offset_to_position(source, end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_multi_line_offsets_map_to_line_and_character() {
        let source = "abc\ndef\nghi";

        // Start of file.
        assert_eq!(offset_to_position(source, 0), Position::new(0, 0));
        // Third character of the first line.
        assert_eq!(offset_to_position(source, 2), Position::new(0, 2));
        // The newline itself still counts as the end of line 0.
        assert_eq!(offset_to_position(source, 3), Position::new(0, 3));
        // First character after the first newline is the start of line 1.
        assert_eq!(offset_to_position(source, 4), Position::new(1, 0));
        // Second character of the third line.
        let offset = source.find("hi").expect("substring present");
        assert_eq!(offset_to_position(source, offset), Position::new(2, 1));
    }

    #[test]
    fn multi_byte_char_before_target_uses_utf16_character_units() {
        // 'é' is two UTF-8 bytes but one UTF-16 code unit; the character
        // column counts UTF-16 units, not bytes.
        let source = "é_x";
        let x_offset = source.find('x').expect("x present");
        assert_eq!(x_offset, 3, "byte offset accounts for the 2-byte é");
        assert_eq!(offset_to_position(source, x_offset), Position::new(0, 2));

        // An emoji outside the BMP is two UTF-16 code units.
        let source = "😀y";
        let y_offset = source.find('y').expect("y present");
        assert_eq!(y_offset, 4, "byte offset accounts for the 4-byte emoji");
        assert_eq!(offset_to_position(source, y_offset), Position::new(0, 2));
    }

    #[test]
    fn offset_at_or_past_end_of_file_clamps_to_final_position() {
        let source = "ab\ncd";

        // Exactly at end-of-file: last line, past its final character.
        assert_eq!(offset_to_position(source, source.len()), Position::new(1, 2));
        // Past end-of-file clamps to the same final position.
        assert_eq!(offset_to_position(source, 999), Position::new(1, 2));
    }

    #[test]
    fn byte_range_maps_both_endpoints() {
        let source = "abc\ndef";
        let range = byte_range_to_lsp(source, 1, 5);
        assert_eq!(range.start, Position::new(0, 1));
        assert_eq!(range.end, Position::new(1, 1));
    }
}
