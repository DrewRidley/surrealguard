//! Position/offset conversion utilities.
//!
//! The analyzer speaks byte offsets; LSP speaks `(line, UTF-16 column)`.
//! Converting one span by scanning the document from byte 0 costs O(n), which
//! is invisible for a single hover and fatal for anything whole-document: a
//! file's semantic tokens, its diagnostics and its inlay hints each convert
//! one span per item, so the whole operation goes quadratic in file size.
//!
//! [`LineIndex`] removes the scan. It records every line's start once per text
//! version; a conversion then binary-searches for the line and counts UTF-16
//! units *inside that line only* — and not even that when the line holds no
//! byte above 0x7F, where the UTF-16 column is exactly the byte column. Build
//! one per document text (the workspace caches it on the [`Document`]) and
//! hand it to every conversion in the batch.
//!
//! [`Document`]: crate::workspace::Document

use std::sync::Arc;

use tower_lsp::lsp_types::{Position, Range};

/// The byte offset of each line start in a document, so a byte offset maps to
/// an LSP position without rescanning the document.
///
/// Holds the text it indexes, so the two can never drift apart. Building one
/// is a single pass; every conversion afterwards is a binary search plus, at
/// most, one line's worth of character counting.
#[derive(Clone)]
pub struct LineIndex {
    /// The indexed text. Shared with the document it came from.
    text: Arc<str>,
    /// Byte offset of each line's first byte. Always starts with 0, so a
    /// document always has at least one line.
    starts: Vec<usize>,
    /// Whether each line (its terminating newline included) is pure ASCII.
    /// For such a line the UTF-16 column *is* the byte column, exactly — one
    /// byte is one code unit — which is the common case in a `.surql` file.
    ascii: Vec<bool>,
}

impl std::fmt::Debug for LineIndex {
    /// Deliberately does not print the text: this hangs off every tracked
    /// document, and a workspace dump should not be a copy of the workspace.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LineIndex")
            .field("lines", &self.starts.len())
            .field("bytes", &self.text.len())
            .finish()
    }
}

impl LineIndex {
    /// Indexes `text`, taking ownership of the shared allocation.
    pub fn new(text: Arc<str>) -> Self {
        let mut starts = Vec::with_capacity(text.len() / 24 + 1);
        starts.push(0usize);
        starts.extend(
            text.match_indices('\n')
                .map(|(offset, newline)| offset + newline.len()),
        );

        let bytes = text.as_bytes();
        let ascii = (0..starts.len())
            .map(|line| {
                let start = starts[line];
                let end = starts.get(line + 1).copied().unwrap_or(bytes.len());
                bytes[start..end].is_ascii()
            })
            .collect();

        Self {
            text,
            starts,
            ascii,
        }
    }

    /// Indexes a borrowed text, copying it. For one-off conversions only —
    /// anything called per token, per finding or per hint should hold a
    /// [`LineIndex`] built once from the document's shared text.
    pub fn for_str(text: &str) -> Self {
        Self::new(Arc::from(text))
    }

    /// The indexed text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The number of lines. Always at least one.
    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// The line containing `offset`, clamped to the last line.
    pub fn line_of(&self, offset: usize) -> usize {
        self.starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1)
    }

    /// The byte offset `line` starts at (the end of the text for a line past
    /// the last).
    pub fn line_start(&self, line: usize) -> usize {
        self.starts.get(line).copied().unwrap_or(self.text.len())
    }

    /// The byte offset one past `line`'s last byte — its terminating newline
    /// included, since that newline belongs to this line, not the next.
    pub fn line_end(&self, line: usize) -> usize {
        self.starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.text.len())
    }

    /// The UTF-16 column of `offset` within its own line.
    pub fn column(&self, offset: usize) -> u32 {
        let offset = self.clamp(offset);
        self.column_in(self.line_of(offset), offset)
    }

    /// Convert a byte offset to an LSP position.
    pub fn offset_to_position(&self, offset: usize) -> Position {
        let offset = self.clamp(offset);
        let line = self.line_of(offset);
        Position::new(line as u32, self.column_in(line, offset))
    }

    /// Convert an LSP position to a byte offset. A column past its line's end
    /// clamps to that line's newline byte; a line past the last clamps to the
    /// end of the text.
    pub fn position_to_offset(&self, position: Position) -> usize {
        let line = position.line as usize;
        let Some(start) = self.starts.get(line).copied() else {
            return self.text.len();
        };
        // The newline is not a column anyone can name, so it is the clamp.
        let end = match self.starts.get(line + 1) {
            Some(next) => next - 1,
            None => self.text.len(),
        };

        if self.ascii[line] {
            return (start + position.character as usize).min(end);
        }

        let mut column = 0u32;
        for (offset, ch) in self.text[start..end].char_indices() {
            if column >= position.character {
                return start + offset;
            }
            column += ch.len_utf16() as u32;
        }
        end
    }

    /// Convert a byte range to an LSP range.
    pub fn range(&self, start: usize, end: usize) -> Range {
        Range {
            start: self.offset_to_position(start),
            end: self.offset_to_position(end),
        }
    }

    /// The UTF-16 column of `offset`, which must lie on `line`.
    ///
    /// The ASCII fast path is exact rather than an approximation: a line with
    /// no byte above 0x7F holds only characters that are one byte in UTF-8 and
    /// one code unit in UTF-16, so the difference of byte offsets *is* the
    /// UTF-16 column.
    fn column_in(&self, line: usize, offset: usize) -> u32 {
        let start = self.line_start(line);
        let offset = offset.max(start);
        if self.ascii.get(line).copied().unwrap_or(false) {
            return (offset - start) as u32;
        }
        self.text[start..offset]
            .chars()
            .map(|ch| ch.len_utf16() as u32)
            .sum()
    }

    /// Clamps an offset into the text and onto a character boundary.
    ///
    /// An offset inside a multi-byte character names the position *after* that
    /// character — the answer the scan this replaces gave, since it counted
    /// every character starting before the target, the last of which contains
    /// it. No span the analyzer produces lands inside a character, so this is
    /// about staying bit-identical, not about a case that arises.
    fn clamp(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset += 1;
        }
        offset
    }
}

/// Convert a byte offset to an LSP Position (line, character).
///
/// Builds an index for `source` on every call; use [`LineIndex`] directly
/// whenever more than one offset of the same text is converted.
pub fn offset_to_position(source: &str, offset: usize) -> Position {
    LineIndex::for_str(source).offset_to_position(offset)
}

/// Convert an LSP Position (line, UTF-16 character) to a byte offset.
/// Clamps past-end positions to the end of their line, or to `source.len()`
/// past the last line.
///
/// Builds an index for `source` on every call; use [`LineIndex`] directly
/// whenever more than one position of the same text is converted.
pub fn position_to_offset(source: &str, position: Position) -> usize {
    LineIndex::for_str(source).position_to_offset(position)
}

/// Convert byte offsets to an LSP Range.
///
/// Builds an index for `source` on every call; use [`LineIndex`] directly
/// whenever more than one range of the same text is converted.
pub fn byte_range_to_lsp(source: &str, start: usize, end: usize) -> Range {
    LineIndex::for_str(source).range(start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The conversion the fast path replaces, kept as the oracle every
    /// `LineIndex` answer is checked against.
    fn scanning_offset_to_position(source: &str, offset: usize) -> Position {
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

    /// The position→offset conversion the fast path replaces.
    fn scanning_position_to_offset(source: &str, position: Position) -> usize {
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
            } else {
                character += ch.len_utf16() as u32;
            }
        }
        source.len()
    }

    /// Every byte offset and every position in a small grid must agree with
    /// the scanning conversion.
    fn agrees_with_a_scan(source: &str) {
        let index = LineIndex::for_str(source);
        for offset in 0..=source.len() {
            assert_eq!(
                index.offset_to_position(offset),
                scanning_offset_to_position(source, offset),
                "offset {offset} of {source:?}"
            );
        }
        for line in 0..(source.lines().count() as u32 + 2) {
            for character in 0..12u32 {
                let position = Position::new(line, character);
                assert_eq!(
                    index.position_to_offset(position),
                    scanning_position_to_offset(source, position),
                    "{position:?} of {source:?}"
                );
            }
        }
    }

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
        assert_eq!(
            offset_to_position(source, source.len()),
            Position::new(1, 2)
        );
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

    #[test]
    fn position_to_offset_is_the_inverse_of_offset_to_position() {
        let source = "LET $age = 42;\nRETURN $age;";
        for offset in [0usize, 4, 8, 15, 22, source.len()] {
            let position = offset_to_position(source, offset);
            assert_eq!(
                position_to_offset(source, position),
                offset,
                "offset {offset}"
            );
        }
    }

    #[test]
    fn position_to_offset_handles_multibyte_and_past_end_columns() {
        // 'é' is one UTF-16 unit but two bytes: column 2 lands on the byte
        // after it.
        let source = "é_x\nyz";
        assert_eq!(position_to_offset(source, Position::new(0, 2)), 3);
        // A column past a line's end clamps to that line's newline byte.
        assert_eq!(position_to_offset(source, Position::new(0, 99)), 4);
        // A line past the last clamps to end of source.
        assert_eq!(
            position_to_offset(source, Position::new(9, 0)),
            source.len()
        );
    }

    #[test]
    fn the_index_records_every_line_start() {
        let index = LineIndex::for_str("a\nbb\n\nccc");
        assert_eq!(index.line_count(), 4);
        assert_eq!(
            (0..index.line_count())
                .map(|line| (index.line_start(line), index.line_end(line)))
                .collect::<Vec<_>>(),
            // Each line's end includes its own newline.
            vec![(0, 2), (2, 5), (5, 6), (6, 9)]
        );
        // An empty text is still one line.
        assert_eq!(LineIndex::for_str("").line_count(), 1);
    }

    #[test]
    fn an_offset_on_a_line_boundary_belongs_to_the_line_it_opens() {
        let source = "SELECT 1;\nSELECT 2;\n";
        let index = LineIndex::for_str(source);
        // The newline is the last column of the line it ends...
        assert_eq!(index.offset_to_position(9), Position::new(0, 9));
        // ...and the byte after it is column 0 of the next.
        assert_eq!(index.offset_to_position(10), Position::new(1, 0));
        // A trailing newline opens a final, empty line.
        assert_eq!(index.offset_to_position(20), Position::new(2, 0));
        assert_eq!(index.position_to_offset(Position::new(1, 0)), 10);
        assert_eq!(index.position_to_offset(Position::new(2, 0)), 20);
    }

    #[test]
    fn the_ascii_fast_path_agrees_with_a_scan() {
        agrees_with_a_scan("SELECT * FROM person;\nRETURN 1;\n\nRETURN 2;");
    }

    #[test]
    fn accented_latin_agrees_with_a_scan() {
        // Two bytes, one UTF-16 unit: the byte column and the character
        // column diverge from the first character on.
        agrees_with_a_scan("LET $café = 'crème';\nRETURN $café;\n");
    }

    #[test]
    fn cjk_agrees_with_a_scan() {
        // Three bytes, one UTF-16 unit.
        agrees_with_a_scan("LET $x = '日本語のテキスト';\nRETURN '中文';\n");
    }

    #[test]
    fn emoji_and_a_zwj_sequence_agree_with_a_scan() {
        // 👩‍💻 is two astral characters (two UTF-16 units each) joined by a
        // zero-width joiner (one unit): five code units, eleven bytes, and
        // one thing the user thinks of as a character. LSP counts code
        // units, so the column after it is 5.
        let source = "RETURN '👩‍💻';\nRETURN '😀🎉';\n";
        agrees_with_a_scan(source);
        let index = LineIndex::for_str(source);
        let after = source.find("';").expect("the literal closes");
        assert_eq!(index.offset_to_position(after), Position::new(0, 13));
        // An offset inside a character names the position after it, never a
        // position the client cannot address.
        let inside = source.find('👩').expect("emoji present") + 1;
        assert_eq!(index.offset_to_position(inside), Position::new(0, 10));
    }

    #[test]
    fn crlf_line_endings_keep_the_carriage_return_on_its_own_line() {
        // The protocol has no column for a line terminator, and the analyzer
        // spans bytes: a `\r` is simply the last character of the line it
        // ends, which is what a scan produced too.
        let source = "SELECT 1;\r\nSELECT 2;\r\n";
        agrees_with_a_scan(source);
        let index = LineIndex::for_str(source);
        assert_eq!(index.line_count(), 3);
        assert_eq!(index.offset_to_position(9), Position::new(0, 9));
        assert_eq!(index.offset_to_position(11), Position::new(1, 0));
        // A column past the line's end clamps to the newline, not past the
        // carriage return.
        assert_eq!(index.position_to_offset(Position::new(0, 99)), 10);
    }

    #[test]
    fn a_position_past_the_last_line_clamps_to_the_end_of_the_text() {
        let source = "RETURN 1;\nRETURN 2;";
        let index = LineIndex::for_str(source);
        assert_eq!(index.position_to_offset(Position::new(7, 0)), source.len());
        assert_eq!(index.position_to_offset(Position::new(1, 99)), source.len());
        // And an offset past the end clamps to the last position.
        assert_eq!(index.offset_to_position(9_999), Position::new(1, 9));
    }

    #[test]
    fn a_mixed_document_agrees_with_a_scan_line_by_line() {
        // The per-line ASCII flag means one line's multi-byte content must
        // not change how any other line converts.
        agrees_with_a_scan("ascii only;\nLET $é = 1;\nascii again;\n😀\nlast;");
    }
}
