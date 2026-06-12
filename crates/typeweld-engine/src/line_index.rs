//! Byte-offset to line/column conversion.
//!
//! Byte offsets are the canonical position representation everywhere in the
//! engine and the language server; conversion to UTF-8 or UTF-16 line/column
//! happens only at protocol or rendering boundaries through this index.

/// Line index over one file's contents.
#[derive(Clone, Debug)]
pub struct LineIndex {
    /// Byte offset of the start of each line (line 0 starts at 0).
    line_starts: Vec<u32>,
    len: u32,
}

impl LineIndex {
    /// Builds the index for `text`.
    #[must_use]
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0_u32];
        for (offset, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(u32::try_from(offset + 1).unwrap_or(u32::MAX));
            }
        }
        Self {
            line_starts,
            len: u32::try_from(text.len()).unwrap_or(u32::MAX),
        }
    }

    /// Number of lines in the file.
    #[must_use]
    pub fn line_count(&self) -> u32 {
        u32::try_from(self.line_starts.len()).unwrap_or(u32::MAX)
    }

    /// Converts a byte offset to a zero-based `(line, utf8_column)` pair.
    ///
    /// Offsets past the end of the file clamp to the last position.
    #[must_use]
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let offset = offset.min(self.len);
        let line = match self.line_starts.binary_search(&offset) {
            Ok(line) => line,
            Err(next_line) => next_line - 1,
        };
        let col = offset - self.line_starts[line];
        (u32::try_from(line).unwrap_or(u32::MAX), col)
    }

    /// Converts a zero-based `(line, utf8_column)` pair back to a byte offset.
    ///
    /// Out-of-range lines clamp to the end of the file; out-of-range columns
    /// clamp to the end of the line.
    #[must_use]
    pub fn offset(&self, line: u32, col: u32, text: &str) -> u32 {
        let Some(&start) = self.line_starts.get(line as usize) else {
            return self.len;
        };
        let line_end = self
            .line_starts
            .get(line as usize + 1)
            .map_or(self.len, |&next| next.saturating_sub(1));
        let target = start.saturating_add(col).min(line_end);
        // Clamp to a char boundary so callers can never split a code point.
        let mut target = target as usize;
        while target > start as usize && !text.is_char_boundary(target) {
            target -= 1;
        }
        u32::try_from(target).unwrap_or(u32::MAX)
    }

    /// Converts a byte offset to a zero-based `(line, utf16_column)` pair.
    #[must_use]
    pub fn line_col_utf16(&self, offset: u32, text: &str) -> (u32, u32) {
        let (line, _) = self.line_col(offset);
        let start = self.line_starts[line as usize] as usize;
        let end = (offset.min(self.len)) as usize;
        let col = text
            .get(start..end)
            .map_or(0, |slice| slice.encode_utf16().count());
        (line, u32::try_from(col).unwrap_or(u32::MAX))
    }

    /// Converts a zero-based `(line, utf16_column)` pair to a byte offset.
    #[must_use]
    pub fn offset_utf16(&self, line: u32, col: u32, text: &str) -> u32 {
        let Some(&start) = self.line_starts.get(line as usize) else {
            return self.len;
        };
        let line_end = self
            .line_starts
            .get(line as usize + 1)
            .map_or(self.len, |&next| next.saturating_sub(1)) as usize;
        let Some(slice) = text.get(start as usize..line_end) else {
            return start;
        };

        let mut utf16_seen: u32 = 0;
        for (byte_offset, character) in slice.char_indices() {
            if utf16_seen >= col {
                return start + u32::try_from(byte_offset).unwrap_or(u32::MAX);
            }
            utf16_seen += u32::try_from(character.len_utf16()).unwrap_or(u32::MAX);
        }
        u32::try_from(line_end).unwrap_or(u32::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_round_trips() {
        let text = "first\nsecond line\n\nlast";
        let index = LineIndex::new(text);

        assert_eq!(index.line_col(0), (0, 0));
        assert_eq!(index.line_col(6), (1, 0));
        assert_eq!(index.line_col(13), (1, 7));
        assert_eq!(index.line_col(18), (2, 0));
        assert_eq!(index.line_col(19), (3, 0));
        // Past the end clamps.
        assert_eq!(index.line_col(1000), (3, 4));

        for offset in [0_u32, 3, 6, 13, 19, 23] {
            let (line, col) = index.line_col(offset);
            assert_eq!(index.offset(line, col, text), offset);
        }
    }

    #[test]
    fn utf16_columns_handle_multibyte_text() {
        // "déjà" is 6 bytes; emoji is 4 bytes / 2 UTF-16 units.
        let text = "déjà x\n🦀 crab";
        let index = LineIndex::new(text);

        // Offset of 'x' on line 0: d(1) é(2) j(1) à(2) space(1) = 7 bytes.
        assert_eq!(index.line_col_utf16(7, text), (0, 5));
        assert_eq!(index.offset_utf16(0, 5, text), 7);

        // Line 1 starts after the newline at byte 9; crab emoji is 4 bytes.
        let crab_line_start = 9;
        assert_eq!(index.line_col_utf16(crab_line_start, text), (1, 0));
        // " crab" after the emoji: byte 13, utf16 col 2.
        assert_eq!(index.line_col_utf16(crab_line_start + 4, text), (1, 2));
        assert_eq!(index.offset_utf16(1, 2, text), crab_line_start + 4);
    }

    #[test]
    fn offset_never_splits_code_points() {
        let text = "🦀🦀";
        let index = LineIndex::new(text);
        let offset = index.offset(0, 2, text);
        assert!(text.is_char_boundary(offset as usize));
    }
}
