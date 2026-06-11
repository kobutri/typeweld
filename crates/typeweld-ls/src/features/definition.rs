//! Cross-language goto-definition.
//!
//! From TypeScript (generated or app code) the definition is the Rust
//! declaration; from Rust it is every generated TypeScript location.

use lsp_types::{GotoDefinitionParams, GotoDefinitionResponse};

use crate::features::{location, resolve, span_location};
use crate::state::State;

pub fn handle(state: &State, params: &GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
    let position = &params.text_document_position_params;
    let resolved = resolve(state, &position.text_document.uri, position.position)?;
    let snapshot = state.snapshot()?;
    let package = &snapshot.packages[resolved.package];

    if resolved.is_from_rust() {
        let mut locations = Vec::new();
        for mark in package
            .marks
            .iter()
            .filter(|mark| mark.symbol_id == resolved.id)
        {
            let path = package.package_dir.join(&mark.file);
            let Some(text) = state.read_text(&path) else {
                continue;
            };
            locations.push(location(
                &path,
                &text,
                mark.start,
                mark.end,
                state.encoding(),
            ));
        }
        return (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations));
    }

    let symbol = package.symbol_by_id(&resolved.id)?;
    span_location(state, &symbol.name_span).map(GotoDefinitionResponse::Scalar)
}
