//! The TypeScript generator: [`Contract`] → tree-shakeable Effect client
//! package.
//!
//! Output layout:
//!
//! ```text
//! package.json            # name, type: module, sideEffects: false
//! schemas.ts              # Schema consts + types (topologically sorted)
//! errors.ts               # TaggedErrorClass per variant + unions + decoders
//! client.ts               # ApiConfig service + layer(config)
//! endpoints/<module>.ts   # one accessor const per endpoint
//! index.ts                # re-exports
//! ```
//!
//! Every accessor pulls its configuration from the `ApiConfig` service and
//! calls an `@typeweld/effect-runtime` client factory, so bundles only
//! contain the endpoints (and schemas) that are actually imported. There is
//! deliberately no namespace object or service-of-all-endpoints.

mod ast;
mod lower;
mod print;

pub use ast::MarkKind;

use serde::{Deserialize, Serialize};

use crate::diag::Diagnostic;
use crate::ir::{Contract, SymbolId};
use crate::line_index::LineIndex;

/// A complete generated package.
pub struct GeneratedPackage {
    pub files: Vec<GeneratedFile>,
    pub marks: Vec<Mark>,
}

/// One generated file, path relative to the package directory.
pub struct GeneratedFile {
    pub path: String,
    pub contents: String,
}

/// The location of a Rust API symbol in the generated TypeScript.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Mark {
    /// Package-relative file path.
    pub file: String,
    pub symbol_id: SymbolId,
    pub kind: MarkKind,
    /// Byte offsets in the generated file.
    pub start: u32,
    pub end: u32,
    /// Zero-based UTF-16 start position (LSP wire format).
    pub line: u32,
    pub character: u32,
}

/// Generates the TypeScript package for `contract`.
///
/// Diagnostics report contract shapes the generator cannot express (e.g.
/// recursive types); the package is still produced for everything else.
#[must_use]
pub fn generate(contract: &Contract) -> (GeneratedPackage, Vec<Diagnostic>) {
    lower::lower(contract)
}

pub(crate) fn finish_marks(
    file: &str,
    contents: &str,
    printed: Vec<print::PrintedMark>,
) -> Vec<Mark> {
    let index = LineIndex::new(contents);
    printed
        .into_iter()
        .map(|printed| {
            let (line, character) = index.line_col_utf16(printed.start, contents);
            Mark {
                file: file.to_owned(),
                symbol_id: printed.mark.symbol_id,
                kind: printed.mark.kind,
                start: printed.start,
                end: printed.end,
                line,
                character,
            }
        })
        .collect()
}

/// Writes the package to `dir`, only touching files whose contents changed,
/// and pruning stale generated `.ts` files.
///
/// # Errors
/// Returns an IO error description on failure.
pub fn write_package(dir: &std::path::Path, package: &GeneratedPackage) -> Result<bool, String> {
    let mut changed = false;
    let mut expected: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();

    for file in &package.files {
        let path = dir.join(&file.path);
        expected.insert(path.clone());
        let current = std::fs::read_to_string(&path).ok();
        if current.as_deref() == Some(file.contents.as_str()) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        std::fs::write(&path, &file.contents)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
        changed = true;
    }

    // Prune generated files that no longer exist in the package.
    for entry in walk_ts_files(dir) {
        if !expected.contains(&entry) {
            let _ = std::fs::remove_file(&entry);
            changed = true;
        }
    }

    Ok(changed)
}

fn walk_ts_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "ts") {
                files.push(path);
            }
        }
    }
    files
}
