//! Bidirectional rename across the contract.
//!
//! Rust-initiated renames edit the Rust declaration, the router mounts, and
//! the TypeScript usages of derived names; TypeScript-initiated renames are
//! mapped back to the Rust name first. Generated files are never edited —
//! they update through regeneration.

use std::collections::BTreeMap;
use std::path::PathBuf;

use lsp_types::{
    DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier, PrepareRenameResponse,
    RenameParams, TextDocumentEdit, TextDocumentPositionParams, TextEdit, WorkspaceEdit,
};
use typeweld_engine::ir::{DeclSpans, Endpoint, ErrorVariant, Span, SymbolId};
use typeweld_engine::usage::{UsageKind, UsageRef};
use typeweld_syntax::{to_camel_case, to_snake_case};

use crate::convert::{self, path_to_uri};
use crate::features::{resolve, trailing_ident};
use crate::state::{PackageSnapshot, State, SymbolInfo};

/// One pending text replacement, by absolute path and byte range.
struct Edit {
    path: PathBuf,
    start: u32,
    end: u32,
    new_text: String,
}

pub fn prepare(
    state: &State,
    params: &TextDocumentPositionParams,
) -> Option<PrepareRenameResponse> {
    let resolved = resolve(state, &params.text_document.uri, params.position)?;
    let text = state.read_text(&resolved.path)?;
    let placeholder = text
        .get(resolved.start as usize..resolved.end as usize)?
        .to_owned();
    let range = convert::range(&text, resolved.start, resolved.end, state.encoding());
    Some(PrepareRenameResponse::RangeWithPlaceholder { range, placeholder })
}

pub fn handle(state: &State, params: &RenameParams) -> Result<Option<WorkspaceEdit>, String> {
    let position = &params.text_document_position;
    let Some(resolved) = resolve(state, &position.text_document.uri, position.position) else {
        return Ok(None);
    };
    let Some(snapshot) = state.snapshot() else {
        return Ok(None);
    };
    let package = &snapshot.packages[resolved.package];
    let Some(symbol) = package.symbol_by_id(&resolved.id) else {
        return Ok(None);
    };

    let new_name = params.new_name.as_str();
    if !is_identifier(new_name) {
        return Err(format!("`{new_name}` is not a valid identifier"));
    }
    let from_rust = resolved.is_from_rust();

    let mut edits = Vec::new();
    match symbol.info {
        SymbolInfo::Endpoint(index) => {
            let endpoint = &package.contract.endpoints[index];
            rename_endpoint(state, package, endpoint, new_name, from_rust, &mut edits)?;
        }
        SymbolInfo::Type(index) => {
            let decl = &package.contract.types[index];
            rename_decl(
                state,
                package,
                &decl.id,
                &decl.spans,
                &decl.rust_name,
                &decl.ts_name,
                new_name,
                &mut edits,
            );
        }
        SymbolInfo::Error(index) => {
            let decl = &package.contract.errors[index];
            rename_decl(
                state,
                package,
                &decl.id,
                &decl.spans,
                &decl.rust_name,
                &decl.ts_name,
                new_name,
                &mut edits,
            );
        }
        SymbolInfo::ErrorVariant { error, variant } => {
            let Some(variant) = package.contract.errors[error].variants.get(variant) else {
                return Ok(None);
            };
            rename_error_variant(state, package, variant, new_name, &mut edits);
        }
        // v1: Rust-only renames. Derived wire names update via regeneration.
        SymbolInfo::Field { .. } | SymbolInfo::Variant { .. } | SymbolInfo::Param { .. } => {
            push_span_edit(state, &symbol.name_span, new_name.to_owned(), &mut edits);
        }
    }

    Ok(Some(build_workspace_edit(state, package, edits)))
}

fn is_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn push_span_edit(state: &State, span: &Span, new_text: String, edits: &mut Vec<Edit>) {
    edits.push(Edit {
        path: state.root().join(&span.file),
        start: span.start,
        end: span.end,
        new_text,
    });
}

/// Edits a TS usage ref, skipping it when the current text does not match the
/// expected old token (e.g. quoted tags or pinned names).
fn push_usage_edit(
    state: &State,
    usage: &UsageRef,
    expected: &str,
    replacement: String,
    edits: &mut Vec<Edit>,
) {
    let path = PathBuf::from(&usage.file);
    let Some(text) = state.read_text(&path) else {
        return;
    };
    if text.get(usage.start as usize..usage.end as usize) != Some(expected) {
        return;
    }
    edits.push(Edit {
        path,
        start: usage.start,
        end: usage.end,
        new_text: replacement,
    });
}

fn rename_endpoint(
    state: &State,
    package: &PackageSnapshot,
    endpoint: &Endpoint,
    new_name: &str,
    from_rust: bool,
    edits: &mut Vec<Edit>,
) -> Result<(), String> {
    let rust_new = if from_rust {
        new_name.to_owned()
    } else {
        let snake = to_snake_case(new_name);
        if to_camel_case(&snake) != new_name {
            return Err(format!(
                "ambiguous casing: `{new_name}` does not round-trip through `{snake}`"
            ));
        }
        snake
    };

    push_span_edit(state, &endpoint.spans.name, rust_new.clone(), edits);
    for mount in &endpoint.router_mounts {
        let path = state.root().join(&mount.file);
        let Some(text) = state.read_text(&path) else {
            continue;
        };
        if let Some((start, end)) = trailing_ident(&text, mount, &endpoint.rust_name) {
            edits.push(Edit {
                path,
                start,
                end,
                new_text: rust_new.clone(),
            });
        }
    }

    let ts_new = to_camel_case(&rust_new);
    let route_old = format!("{}Route", endpoint.ts_name);
    for usage in package.usage.refs_for(&endpoint.id) {
        match usage.kind {
            UsageKind::Endpoint => {
                push_usage_edit(state, usage, &endpoint.ts_name, ts_new.clone(), edits);
            }
            UsageKind::Route => {
                push_usage_edit(state, usage, &route_old, format!("{ts_new}Route"), edits);
            }
            _ => {}
        }
    }
    Ok(())
}

/// Renames a type or error declaration: the name is the same on both sides,
/// so TS usages are edited only when the TS name was derived (identical).
#[allow(clippy::too_many_arguments)]
fn rename_decl(
    state: &State,
    package: &PackageSnapshot,
    id: &SymbolId,
    spans: &DeclSpans,
    rust_name: &str,
    ts_name: &str,
    new_name: &str,
    edits: &mut Vec<Edit>,
) {
    push_span_edit(state, &spans.name, new_name.to_owned(), edits);
    if ts_name != rust_name {
        return;
    }
    for usage in package.usage.refs_for(id) {
        if matches!(usage.kind, UsageKind::Type | UsageKind::Error) {
            push_usage_edit(state, usage, ts_name, new_name.to_owned(), edits);
        }
    }
}

fn rename_error_variant(
    state: &State,
    package: &PackageSnapshot,
    variant: &ErrorVariant,
    new_name: &str,
    edits: &mut Vec<Edit>,
) {
    push_span_edit(state, &variant.spans.name, new_name.to_owned(), edits);
    // Only derived tags follow the Rust name; explicit serde renames pin the
    // wire tag, so TS stays untouched.
    let new_tag = if variant.tag == variant.rust_name {
        new_name.to_owned()
    } else if variant.tag == to_camel_case(&variant.rust_name) {
        to_camel_case(new_name)
    } else {
        return;
    };
    for usage in package.usage.refs_for(&variant.id) {
        if matches!(usage.kind, UsageKind::ErrorVariant | UsageKind::ErrorTag) {
            push_usage_edit(state, usage, &variant.tag, new_tag.clone(), edits);
        }
    }
}

fn build_workspace_edit(
    state: &State,
    package: &PackageSnapshot,
    edits: Vec<Edit>,
) -> WorkspaceEdit {
    let mut by_file: BTreeMap<PathBuf, Vec<Edit>> = BTreeMap::new();
    let mut seen = std::collections::HashSet::new();
    for edit in edits {
        // Never edit generated files.
        if edit.path.starts_with(&package.package_dir) {
            continue;
        }
        if !seen.insert((edit.path.clone(), edit.start, edit.end)) {
            continue;
        }
        by_file.entry(edit.path.clone()).or_default().push(edit);
    }

    let mut documents = Vec::new();
    for (path, edits) in by_file {
        let Some(text) = state.read_text(&path) else {
            continue;
        };
        let text_edits = edits
            .into_iter()
            .map(|edit| {
                OneOf::Left(TextEdit {
                    range: convert::range(&text, edit.start, edit.end, state.encoding()),
                    new_text: edit.new_text,
                })
            })
            .collect();
        documents.push(TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: path_to_uri(&path),
                version: state.version_of(&path),
            },
            edits: text_edits,
        });
    }

    WorkspaceEdit {
        changes: None,
        document_changes: Some(DocumentChanges::Edits(documents)),
        change_annotations: None,
    }
}
