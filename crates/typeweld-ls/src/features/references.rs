//! Cross-language find-references.
//!
//! The union of the Rust declaration, router mounts (for endpoints), and the
//! TypeScript usage refs. Generated files are never reported.

use std::path::PathBuf;

use lsp_types::{Location, ReferenceParams};

use crate::features::{location, resolve, span_location, trailing_ident};
use crate::state::{State, SymbolInfo};

pub fn handle(
    state: &State,
    plugin: Option<&crate::plugin::PluginHub>,
    params: &ReferenceParams,
) -> Option<Vec<Location>> {
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

    locations.extend(ts_usage_locations(state, plugin, package, &resolved.id));
    (!locations.is_empty()).then_some(locations)
}

/// The TypeScript usage locations of the API symbol at a position — the
/// cross-language half merged into rust-analyzer's references response.
pub fn ts_locations(
    state: &State,
    plugin: Option<&crate::plugin::PluginHub>,
    uri: &lsp_types::Uri,
    position: lsp_types::Position,
) -> Vec<Location> {
    let Some(resolved) = resolve(state, uri, position) else {
        return Vec::new();
    };
    let Some(snapshot) = state.snapshot() else {
        return Vec::new();
    };
    let package = &snapshot.packages[resolved.package];
    ts_usage_locations(state, plugin, package, &resolved.id)
}

/// Statically indexed usages plus — for fields and params — the semantic
/// property accesses the plugin sees inside the user's tsserver.
fn ts_usage_locations(
    state: &State,
    plugin: Option<&crate::plugin::PluginHub>,
    package: &crate::state::PackageSnapshot,
    id: &typeweld_engine::ir::SymbolId,
) -> Vec<Location> {
    let mut locations = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |path: &std::path::Path, start: u32, end: u32, state: &State| {
        let Some(text) = state.read_text(path) else {
            return;
        };
        if seen.insert((path.to_path_buf(), start, end)) {
            locations.push(location(path, &text, start, end, state.encoding()));
        }
    };
    for usage in package.usage.refs_for(id) {
        let path = PathBuf::from(&usage.file);
        push(&path, usage.start, usage.end, state);
    }
    if let Some(symbol) = package.symbol_by_id(id) {
        for (path, start, end) in
            crate::features::property_locations(state, plugin, package, symbol)
        {
            push(&path, start, end, state);
        }
    }
    locations
}
