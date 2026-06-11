//! Cross-language find-references.
//!
//! The union of the Rust declaration, router mounts (for endpoints), and the
//! TypeScript usage refs. Generated files are never reported.

use std::path::PathBuf;
use std::time::Duration;

use lsp_types::{Location, ReferenceParams};
use typeweld_engine::gen::MarkKind;

use crate::convert;
use crate::features::{location, resolve, span_location, trailing_ident};
use crate::plugin::PluginHub;
use crate::state::{State, SymbolInfo};

pub fn handle(
    state: &State,
    plugin: Option<&PluginHub>,
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

    locations.extend(usage_locations(state, package, &resolved.id));
    locations.extend(ts_property_locations(state, plugin, package, &resolved.id));
    (!locations.is_empty()).then_some(locations)
}

/// The TypeScript usage locations of the API symbol at a position — the
/// cross-language half merged into rust-analyzer's references response.
pub fn ts_locations(
    state: &State,
    plugin: Option<&PluginHub>,
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
    let mut locations = usage_locations(state, package, &resolved.id);
    locations.extend(ts_property_locations(state, plugin, package, &resolved.id));
    locations
}

fn usage_locations(
    state: &State,
    package: &crate::state::PackageSnapshot,
    id: &typeweld_engine::ir::SymbolId,
) -> Vec<Location> {
    let mut locations = Vec::new();
    for usage in package.usage.refs_for(id) {
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
    locations
}

pub(super) struct TsPropertySpan {
    pub(super) path: PathBuf,
    pub(super) start: u32,
    pub(super) end: u32,
}

fn ts_property_locations(
    state: &State,
    plugin: Option<&PluginHub>,
    package: &crate::state::PackageSnapshot,
    id: &typeweld_engine::ir::SymbolId,
) -> Vec<Location> {
    let mut locations = Vec::new();
    for span in ts_property_spans(state, plugin, package, id) {
        let Some(text) = state.read_text(&span.path) else {
            continue;
        };
        locations.push(location(
            &span.path,
            &text,
            span.start,
            span.end,
            state.encoding(),
        ));
    }
    locations
}

pub(super) fn ts_property_spans(
    state: &State,
    plugin: Option<&PluginHub>,
    package: &crate::state::PackageSnapshot,
    id: &typeweld_engine::ir::SymbolId,
) -> Vec<TsPropertySpan> {
    let Some(plugin) = plugin else {
        return Vec::new();
    };
    let Some(symbol) = package.symbol_by_id(id) else {
        return Vec::new();
    };
    let kind = match symbol.info {
        SymbolInfo::Field { .. } => MarkKind::Field,
        SymbolInfo::Param { .. } => MarkKind::Param,
        _ => return Vec::new(),
    };
    let Some(mark) = package
        .marks
        .iter()
        .find(|mark| mark.kind == kind && mark.symbol_id == *id)
    else {
        return Vec::new();
    };
    let anchor = package.package_dir.join(&mark.file);
    let Some(anchor_text) = state.read_text(&anchor) else {
        return Vec::new();
    };
    let offset = convert::utf16_offset(&anchor_text, mark.start);
    let locations = match plugin.query_rename_locations(&anchor, offset, plugin_timeout()) {
        Ok(locations) => locations,
        Err(message) => {
            eprintln!("typeweld-ls: TypeScript property locations unavailable: {message}");
            return Vec::new();
        }
    };
    let Some(snapshot) = state.snapshot() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for plugin_location in locations {
        let path = PathBuf::from(&plugin_location.file);
        let generated = snapshot
            .packages
            .iter()
            .any(|package| path.starts_with(&package.package_dir));
        let is_ts = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension == "ts" || extension == "tsx");
        if generated || !is_ts {
            continue;
        }
        let Some(text) = state.read_text(&path) else {
            continue;
        };
        let start = convert::byte_offset_from_utf16(&text, plugin_location.start);
        let end =
            convert::byte_offset_from_utf16(&text, plugin_location.start + plugin_location.length);
        out.push(TsPropertySpan { path, start, end });
    }
    out
}

fn plugin_timeout() -> Duration {
    std::env::var("TYPEWELD_PLUGIN_TIMEOUT_SECS")
        .ok()
        .and_then(|seconds| seconds.parse().ok())
        .map_or(Duration::from_secs(3), Duration::from_secs)
}
