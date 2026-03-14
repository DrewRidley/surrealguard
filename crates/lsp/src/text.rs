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

/// Convert an LSP Position to a byte offset.
pub fn position_to_offset(source: &str, position: Position) -> usize {
    let mut line = 0u32;
    let mut character = 0u32;

    for (byte, ch) in source.char_indices() {
        if line == position.line && character >= position.character {
            return byte;
        }
        if ch == '\n' {
            if line == position.line {
                return byte;
            }
            line += 1;
            character = 0;
        } else if line == position.line {
            character += ch.len_utf16() as u32;
        }
    }

    source.len()
}

/// Convert byte offsets to an LSP Range.
pub fn byte_range_to_lsp(source: &str, start: usize, end: usize) -> Range {
    Range {
        start: offset_to_position(source, start),
        end: offset_to_position(source, end),
    }
}
