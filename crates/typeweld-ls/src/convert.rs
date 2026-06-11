//! Conversions at the protocol boundary.
//!
//! Byte offsets are the canonical position representation everywhere in the
//! engine; LSP positions and URIs are produced and consumed only here.

use std::path::{Path, PathBuf};

use lsp_types::{Position, Range, Uri};
use typeweld_engine::line_index::LineIndex;

/// Negotiated position encoding for the session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Encoding {
    Utf8,
    Utf16,
}

/// Converts an absolute path to a `file://` URI.
pub fn path_to_uri(path: &Path) -> Uri {
    let mut encoded = String::from("file://");
    let text = path.to_string_lossy();
    if !text.starts_with('/') {
        encoded.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                encoded.push(char::from(byte));
            }
            other => {
                encoded.push('%');
                encoded.push(char::from_digit(u32::from(other >> 4), 16).unwrap_or('0'));
                encoded.push(char::from_digit(u32::from(other & 0xf), 16).unwrap_or('0'));
            }
        }
    }
    encoded
        .parse()
        .unwrap_or_else(|_| "file:///".parse().expect("static URI parses"))
}

/// Converts a `file://` URI back to an absolute path.
pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let rest = uri.as_str().strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    percent_decode(rest).map(PathBuf::from)
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let digits = input.get(index + 1..index + 3)?;
            decoded.push(u8::from_str_radix(digits, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// Converts an LSP position to a byte offset in `text`.
pub fn offset_at(text: &str, position: Position, encoding: Encoding) -> u32 {
    let index = LineIndex::new(text);
    match encoding {
        Encoding::Utf8 => index.offset(position.line, position.character, text),
        Encoding::Utf16 => index.offset_utf16(position.line, position.character, text),
    }
}

/// Converts a byte offset in `text` to an LSP position.
pub fn position_at(text: &str, offset: u32, encoding: Encoding) -> Position {
    let index = LineIndex::new(text);
    let (line, character) = match encoding {
        Encoding::Utf8 => index.line_col(offset),
        Encoding::Utf16 => index.line_col_utf16(offset, text),
    };
    Position::new(line, character)
}

/// Converts a byte range in `text` to an LSP range.
pub fn range(text: &str, start: u32, end: u32, encoding: Encoding) -> Range {
    Range::new(
        position_at(text, start, encoding),
        position_at(text, end, encoding),
    )
}

/// Converts a byte offset into a UTF-16 code-unit offset over the whole
/// text (tsserver's position representation).
pub fn utf16_offset(text: &str, byte_offset: u32) -> u32 {
    let prefix = text.get(..byte_offset as usize).unwrap_or(text);
    prefix
        .chars()
        .map(|character| if character.len_utf16() == 2 { 2 } else { 1 })
        .sum()
}

/// Converts a UTF-16 code-unit offset back into a byte offset.
pub fn byte_offset_from_utf16(text: &str, utf16: u32) -> u32 {
    let mut remaining = utf16;
    for (byte, character) in text.char_indices() {
        let byte = u32::try_from(byte).unwrap_or(u32::MAX);
        if remaining == 0 {
            return byte;
        }
        let width = if character.len_utf16() == 2 { 2 } else { 1 };
        if width > remaining {
            return byte;
        }
        remaining -= width;
    }
    u32::try_from(text.len()).unwrap_or(u32::MAX)
}

/// Renders `path` relative to `root` with `/` separators, matching the
/// `ir::Span::file` convention.
pub fn root_relative(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    Some(relative.to_string_lossy().replace('\\', "/"))
}
