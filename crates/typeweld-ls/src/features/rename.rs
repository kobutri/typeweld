//! Bidirectional rename across the contract.
//!
//! Rust-initiated renames are answered by rust-analyzer (through the proxy);
//! [`ts_complement`] contributes the TypeScript half: usage-index edits for
//! endpoints, types, errors, and tags, plus semantic property-access edits
//! from the TypeScript plugin for fields and params. TypeScript-initiated
//! renames arrive as plugin events; [`rust_complement`] maps them back to the
//! Rust name and plans the Rust-side edit. Generated files are never edited —
//! they update through regeneration.
//!
//! Without rust-analyzer the degraded [`handle`] path produces the
//! contract-known edits (declaration ident, router mounts) itself.

use std::collections::BTreeMap;
use std::path::PathBuf;

use lsp_types::{
    DocumentChangeOperation, DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier,
    Position, PrepareRenameResponse, RenameParams, TextDocumentEdit, TextDocumentPositionParams,
    TextEdit, WorkspaceEdit,
};
use typeweld_engine::ir::{Endpoint, ErrorVariant, Span, SymbolId};
use typeweld_engine::usage::{UsageKind, UsageRef};
use typeweld_syntax::{to_camel_case, to_snake_case};

use crate::convert::{self, path_to_uri, uri_to_path};
use crate::features::{references, resolve, trailing_ident};
use crate::plugin::PluginHub;
use crate::state::{FieldOwner, PackageSnapshot, RustSymbol, Snapshot, State, SymbolInfo};

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

/// The degraded-mode rename: contract-known Rust edits plus the TypeScript
/// complement, without semantic Rust coverage.
pub fn handle(
    state: &State,
    plugin: Option<&PluginHub>,
    params: &RenameParams,
) -> Result<Option<WorkspaceEdit>, String> {
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
            let rust_new = endpoint_rust_name(endpoint, new_name, from_rust)?;
            push_span_edit(state, &endpoint.spans.name, rust_new.clone(), &mut edits);
            push_mount_edits(state, endpoint, &rust_new, &mut edits);
            endpoint_ts_edits(state, package, endpoint, &rust_new, &mut edits);
        }
        SymbolInfo::Type(index) => {
            let decl = &package.contract.types[index];
            push_span_edit(state, &decl.spans.name, new_name.to_owned(), &mut edits);
            decl_ts_edits(
                state,
                package,
                &decl.id,
                &decl.rust_name,
                &decl.ts_name,
                new_name,
                &mut edits,
            );
        }
        SymbolInfo::Error(index) => {
            let decl = &package.contract.errors[index];
            push_span_edit(state, &decl.spans.name, new_name.to_owned(), &mut edits);
            decl_ts_edits(
                state,
                package,
                &decl.id,
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
            push_span_edit(state, &variant.spans.name, new_name.to_owned(), &mut edits);
            variant_tag_edits(state, package, variant, new_name, &mut edits);
        }
        // The Rust ident edit; derived wire names in the generated files
        // update via regeneration, and the plugin below chases the user
        // property accesses (fields, params) the contract cannot see.
        SymbolInfo::Field { .. } | SymbolInfo::Variant { .. } | SymbolInfo::Param { .. } => {
            push_span_edit(state, &symbol.name_span, new_name.to_owned(), &mut edits);
            property_edits(
                state, plugin, package, symbol, new_name, from_rust, &mut edits,
            );
        }
    }

    Ok(Some(build_workspace_edit(state, package, edits)))
}

/// The TypeScript half of a Rust-initiated rename, as its own workspace
/// edit (merged into rust-analyzer's response by the proxy). `None` when the
/// position is not an API symbol or the rename does not touch TypeScript.
pub fn ts_complement(
    state: &State,
    plugin: Option<&PluginHub>,
    params: &RenameParams,
    rust_edit: Option<&WorkspaceEdit>,
) -> Option<WorkspaceEdit> {
    let snapshot = state.snapshot()?;
    let target = ts_complement_target(state, params, rust_edit)?;
    let package = &snapshot.packages[target.package];
    let symbol = package.symbol_by_id(&target.symbol)?;
    let new_name = params.new_name.as_str();
    if !is_identifier(new_name) {
        return None;
    }

    let mut edits = Vec::new();
    push_ts_complement_edits(state, plugin, package, symbol, new_name, &mut edits);
    if edits.is_empty() {
        return None;
    }
    Some(build_workspace_edit(state, package, edits))
}

struct Target {
    package: usize,
    symbol: SymbolId,
}

fn ts_complement_target(
    state: &State,
    params: &RenameParams,
    rust_edit: Option<&WorkspaceEdit>,
) -> Option<Target> {
    let position = &params.text_document_position;
    if let Some(resolved) = resolve(state, &position.text_document.uri, position.position) {
        if resolved.is_from_rust() {
            return Some(Target {
                package: resolved.package,
                symbol: resolved.id,
            });
        }
    }
    rust_edit.and_then(|edit| target_from_rust_edit(state, edit))
}

fn target_from_rust_edit(state: &State, edit: &WorkspaceEdit) -> Option<Target> {
    let snapshot = state.snapshot()?;
    if let Some(changes) = &edit.changes {
        for (uri, text_edits) in changes {
            for text_edit in text_edits {
                if let Some(target) = target_from_text_edit(state, snapshot, uri, text_edit) {
                    return Some(target);
                }
            }
        }
    }
    if let Some(document_changes) = &edit.document_changes {
        match document_changes {
            DocumentChanges::Edits(documents) => {
                for document in documents {
                    for text_edit in &document.edits {
                        if let Some(target) =
                            target_from_document_text_edit(state, snapshot, document, text_edit)
                        {
                            return Some(target);
                        }
                    }
                }
            }
            DocumentChanges::Operations(operations) => {
                for operation in operations {
                    if let DocumentChangeOperation::Edit(document) = operation {
                        for text_edit in &document.edits {
                            if let Some(target) =
                                target_from_document_text_edit(state, snapshot, document, text_edit)
                            {
                                return Some(target);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn target_from_document_text_edit(
    state: &State,
    snapshot: &Snapshot,
    document: &TextDocumentEdit,
    text_edit: &OneOf<TextEdit, lsp_types::AnnotatedTextEdit>,
) -> Option<Target> {
    match text_edit {
        OneOf::Left(text_edit) => {
            target_from_text_edit(state, snapshot, &document.text_document.uri, text_edit)
        }
        OneOf::Right(annotated) => target_from_text_edit(
            state,
            snapshot,
            &document.text_document.uri,
            &annotated.text_edit,
        ),
    }
}

fn target_from_text_edit(
    state: &State,
    snapshot: &Snapshot,
    uri: &lsp_types::Uri,
    text_edit: &TextEdit,
) -> Option<Target> {
    let path = uri_to_path(uri)?;
    let text = state.read_text(&path)?;
    for (package, package_snapshot) in snapshot.packages.iter().enumerate() {
        for symbol in &package_snapshot.symbols {
            if path != state.root().join(&symbol.name_span.file) {
                continue;
            }
            let range = convert::range(
                &text,
                symbol.name_span.start,
                symbol.name_span.end,
                state.encoding(),
            );
            if text_edit.range == range {
                return Some(Target {
                    package,
                    symbol: symbol.id.clone(),
                });
            }
        }
    }
    None
}

fn push_ts_complement_edits(
    state: &State,
    plugin: Option<&PluginHub>,
    package: &PackageSnapshot,
    symbol: &RustSymbol,
    new_name: &str,
    edits: &mut Vec<Edit>,
) {
    match symbol.info {
        SymbolInfo::Endpoint(index) => {
            let endpoint = &package.contract.endpoints[index];
            endpoint_ts_edits(state, package, endpoint, new_name, edits);
        }
        SymbolInfo::Type(index) => {
            let decl = &package.contract.types[index];
            decl_ts_edits(
                state,
                package,
                &decl.id,
                &decl.rust_name,
                &decl.ts_name,
                new_name,
                edits,
            );
        }
        SymbolInfo::Error(index) => {
            let decl = &package.contract.errors[index];
            decl_ts_edits(
                state,
                package,
                &decl.id,
                &decl.rust_name,
                &decl.ts_name,
                new_name,
                edits,
            );
        }
        SymbolInfo::ErrorVariant { error, variant } => {
            let Some(variant) = package.contract.errors[error].variants.get(variant) else {
                return;
            };
            variant_tag_edits(state, package, variant, new_name, edits);
        }
        SymbolInfo::Field { .. } | SymbolInfo::Variant { .. } | SymbolInfo::Param { .. } => {
            property_edits(state, plugin, package, symbol, new_name, true, edits);
        }
    }
}

/// The planned Rust complement of a TypeScript-initiated rename.
pub struct RustComplement {
    /// The new Rust-side name.
    pub rust_name: String,
    /// Where to ask rust-analyzer for the semantic rename.
    pub decl_position: Option<(PathBuf, Position)>,
    /// The contract-known Rust edits, applied when no rust-analyzer runs.
    pub fallback_edit: WorkspaceEdit,
}

/// Maps a TypeScript-initiated rename (reported by the plugin) back to the
/// Rust side. `None` when the symbol is unknown, the wire name is pinned by
/// an explicit serde rename, or the new name cannot round-trip.
pub fn rust_complement(state: &State, symbol: &str, ts_new: &str) -> Option<RustComplement> {
    if !is_identifier(ts_new) {
        return None;
    }
    let id = SymbolId::new(symbol);
    let snapshot = state.snapshot()?;
    let (package, found) = snapshot
        .packages
        .iter()
        .find_map(|package| package.symbol_by_id(&id).map(|symbol| (package, symbol)))?;

    let rust_name = match found.info {
        SymbolInfo::Endpoint(_) => {
            let snake = to_snake_case(ts_new);
            (to_camel_case(&snake) == ts_new).then_some(snake)?
        }
        SymbolInfo::Type(index) => {
            let decl = &package.contract.types[index];
            (decl.ts_name == decl.rust_name).then(|| ts_new.to_owned())?
        }
        SymbolInfo::Error(index) => {
            let decl = &package.contract.errors[index];
            (decl.ts_name == decl.rust_name).then(|| ts_new.to_owned())?
        }
        SymbolInfo::Field { owner, field } => {
            let field = field_decl(package, owner, field)?;
            rust_name_from_wire(&field.rust_name, &field.wire_name, ts_new)?
        }
        SymbolInfo::Param {
            endpoint,
            query,
            param,
        } => {
            let request = &package.contract.endpoints.get(endpoint)?.request;
            let param = if query {
                request.query_params.get(param)
            } else {
                request.path_params.get(param)
            }?;
            let rust_ident = span_text(state, &found.name_span)?;
            rust_name_from_wire(&rust_ident, &param.name, ts_new)?
        }
        SymbolInfo::Variant { .. } | SymbolInfo::ErrorVariant { .. } => return None,
    };
    if !is_identifier(&rust_name) {
        return None;
    }

    let decl_path = state.root().join(&found.name_span.file);
    let decl_position = state.read_text(&decl_path).map(|text| {
        let position = convert::position_at(&text, found.name_span.start, state.encoding());
        (decl_path.clone(), position)
    });

    let mut edits = Vec::new();
    push_span_edit(state, &found.name_span, rust_name.clone(), &mut edits);
    if let SymbolInfo::Endpoint(index) = found.info {
        push_mount_edits(
            state,
            &package.contract.endpoints[index],
            &rust_name,
            &mut edits,
        );
    }
    let fallback_edit = build_workspace_edit(state, package, edits);

    Some(RustComplement {
        rust_name,
        decl_position,
        fallback_edit,
    })
}

/// The Rust ident behind a wire name and a new TypeScript spelling; `None`
/// when the wire name is pinned by an explicit serde rename.
fn rust_name_from_wire(rust_name: &str, wire_name: &str, ts_new: &str) -> Option<String> {
    if wire_name == rust_name {
        Some(ts_new.to_owned())
    } else if wire_name == to_camel_case(rust_name) {
        let snake = to_snake_case(ts_new);
        (to_camel_case(&snake) == ts_new).then_some(snake)
    } else {
        None
    }
}

/// Keeps only the Rust-file edits of a workspace edit (the TypeScript side
/// of a TypeScript-initiated rename was already applied by the editor).
#[must_use]
pub fn filter_to_rust(edit: WorkspaceEdit) -> WorkspaceEdit {
    let is_rust_uri = |uri: &lsp_types::Uri| {
        uri_to_path(uri).is_some_and(|path| {
            path.extension().and_then(|extension| extension.to_str()) == Some("rs")
        })
    };
    let changes = edit.changes.map(|changes| {
        changes
            .into_iter()
            .filter(|(uri, _)| is_rust_uri(uri))
            .collect()
    });
    let document_changes = edit.document_changes.map(|documents| match documents {
        DocumentChanges::Edits(documents) => DocumentChanges::Edits(
            documents
                .into_iter()
                .filter(|document| is_rust_uri(&document.text_document.uri))
                .collect(),
        ),
        DocumentChanges::Operations(operations) => DocumentChanges::Operations(
            operations
                .into_iter()
                .filter(|operation| match operation {
                    DocumentChangeOperation::Edit(document) => {
                        is_rust_uri(&document.text_document.uri)
                    }
                    DocumentChangeOperation::Op(_) => false,
                })
                .collect(),
        ),
    });
    WorkspaceEdit {
        changes,
        document_changes,
        change_annotations: None,
    }
}

/// The new Rust name for an endpoint rename (mapped back from camelCase when
/// the rename originated in TypeScript).
fn endpoint_rust_name(
    _endpoint: &Endpoint,
    new_name: &str,
    from_rust: bool,
) -> Result<String, String> {
    if from_rust {
        return Ok(new_name.to_owned());
    }
    let snake = to_snake_case(new_name);
    if to_camel_case(&snake) != new_name {
        return Err(format!(
            "ambiguous casing: `{new_name}` does not round-trip through `{snake}`"
        ));
    }
    Ok(snake)
}

/// Router-mount edits for an endpoint rename.
fn push_mount_edits(state: &State, endpoint: &Endpoint, rust_new: &str, edits: &mut Vec<Edit>) {
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
                new_text: rust_new.to_owned(),
            });
        }
    }
}

/// TypeScript usage edits for an endpoint rename: accessor and route names.
fn endpoint_ts_edits(
    state: &State,
    package: &PackageSnapshot,
    endpoint: &Endpoint,
    rust_new: &str,
    edits: &mut Vec<Edit>,
) {
    let ts_new = to_camel_case(rust_new);
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
}

/// TypeScript usage edits for a type or error rename: the name is the same on
/// both sides, so usages are edited only when the TS name was derived.
fn decl_ts_edits(
    state: &State,
    package: &PackageSnapshot,
    id: &SymbolId,
    rust_name: &str,
    ts_name: &str,
    new_name: &str,
    edits: &mut Vec<Edit>,
) {
    if ts_name != rust_name {
        return;
    }
    for usage in package.usage.refs_for(id) {
        if matches!(usage.kind, UsageKind::Type | UsageKind::Error) {
            push_usage_edit(state, usage, ts_name, new_name.to_owned(), edits);
        }
    }
}

/// TypeScript tag edits for an error-variant rename. Only derived tags follow
/// the Rust name; explicit serde renames pin the wire tag.
fn variant_tag_edits(
    state: &State,
    package: &PackageSnapshot,
    variant: &ErrorVariant,
    new_name: &str,
    edits: &mut Vec<Edit>,
) {
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

/// Semantic property-access edits for a field or param rename, from the
/// TypeScript plugin running inside the user's tsserver. The plugin is asked
/// at the generated anchor property; its locations are filtered to user
/// files. Failures only cost TypeScript coverage.
fn property_edits(
    state: &State,
    plugin: Option<&PluginHub>,
    package: &PackageSnapshot,
    symbol: &RustSymbol,
    new_name: &str,
    from_rust: bool,
    edits: &mut Vec<Edit>,
) {
    let Some(ts_new) = ts_property_target(state, package, symbol, new_name, from_rust) else {
        return;
    };
    for span in references::ts_property_spans(state, plugin, package, &symbol.id) {
        edits.push(Edit {
            path: span.path,
            start: span.start,
            end: span.end,
            new_text: ts_new.clone(),
        });
    }
}

/// The new TypeScript property name for a field or param symbol when the
/// wire name is derived from the Rust ident. Variants stay Rust-only (v1).
fn ts_property_target(
    state: &State,
    package: &PackageSnapshot,
    symbol: &RustSymbol,
    new_name: &str,
    from_rust: bool,
) -> Option<String> {
    match symbol.info {
        SymbolInfo::Field { owner, field } => {
            let field = field_decl(package, owner, field)?;
            derived_property(&field.rust_name, &field.wire_name, new_name, from_rust)
        }
        SymbolInfo::Param {
            endpoint,
            query,
            param,
        } => {
            let request = &package.contract.endpoints.get(endpoint)?.request;
            let param = if query {
                request.query_params.get(param)
            } else {
                request.path_params.get(param)
            }?;
            // Params store only the wire name; the Rust ident is read from
            // the declaration span to detect explicit serde renames.
            let rust_ident = span_text(state, &symbol.name_span)?;
            derived_property(&rust_ident, &param.name, new_name, from_rust)
        }
        _ => None,
    }
}

/// The current text under a workspace-root-relative span.
fn span_text(state: &State, span: &Span) -> Option<String> {
    let text = state.read_text(&state.root().join(&span.file))?;
    Some(text.get(span.start as usize..span.end as usize)?.to_owned())
}

/// Looks up the field declaration behind a [`SymbolInfo::Field`].
fn field_decl(
    package: &PackageSnapshot,
    owner: FieldOwner,
    field: usize,
) -> Option<&typeweld_engine::ir::Field> {
    use typeweld_engine::ir::TypeShape;
    match owner {
        FieldOwner::Struct(ty) => match &package.contract.types.get(ty)?.shape {
            TypeShape::Struct { fields } => fields.get(field),
            _ => None,
        },
        FieldOwner::Enum { ty, variant } => match &package.contract.types.get(ty)?.shape {
            TypeShape::Enum { variants } => variants.get(variant)?.fields.get(field),
            _ => None,
        },
        FieldOwner::Error { error, variant } => package
            .contract
            .errors
            .get(error)?
            .variants
            .get(variant)?
            .fields
            .get(field),
    }
}

/// The new TypeScript property name when the wire name is derived from the
/// Rust ident; `None` when an explicit serde rename pins the wire name (the
/// generated property — and therefore the user code — must not change).
///
/// Rust-initiated renames carry the new Rust ident, so the wire derivation is
/// re-applied; TS-initiated renames already carry the TS spelling.
fn derived_property(
    rust_name: &str,
    wire_name: &str,
    new_name: &str,
    from_rust: bool,
) -> Option<String> {
    if wire_name == rust_name {
        Some(new_name.to_owned())
    } else if wire_name == to_camel_case(rust_name) {
        Some(if from_rust {
            to_camel_case(new_name)
        } else {
            new_name.to_owned()
        })
    } else {
        None
    }
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
