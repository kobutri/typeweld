//! Cross-language find-references.
//!
//! The union of the Rust declaration, router mounts (for endpoints), and the
//! TypeScript usage refs. Generated files are never reported.

use std::path::PathBuf;

use lsp_types::{Location, ReferenceParams};

use crate::features::{location, resolve, span_location, trailing_ident};
use crate::state::{State, SymbolInfo};

pub fn handle(state: &State, params: &ReferenceParams) -> Option<Vec<Location>> {
    let position = &params.text_document_position;
    let resolved = resolve(state, &position.text_document.uri, position.position)?;
    let snapshot = state.snapshot()?;
    let package = &snapshot.packages[resolved.package];

    let mut locations = Vec::new();
    if let Some(symbol) = package.symbol_by_id(&resolved.id) {
        if let Some(declaration) = span_location(state, &symbol.name_span) {
            locations.push(declaration);
        }
        if let SymbolInfo::Endpoint(index) = symbol.info {
            let endpoint = &package.contract.endpoints[index];
            for mount in &endpoint.router_mounts {
                let path = state.root().join(&mount.file);
                let Some(text) = state.read_text(&path) else {
                    continue;
                };
                let (start, end) = trailing_ident(&text, mount, &endpoint.rust_name)
                    .unwrap_or((mount.start, mount.end));
                locations.push(location(&path, &text, start, end, state.encoding()));
            }
        }
    }

    for usage in package.usage.refs_for(&resolved.id) {
        let path = PathBuf::from(&usage.file);
        let Some(text) = state.read_text(&path) else {
            continue;
        };
        locations.push(location(
            &path,
            &text,
            usage.start,
            usage.end,
            state.encoding(),
        ));
    }

    (!locations.is_empty()).then_some(locations)
}
