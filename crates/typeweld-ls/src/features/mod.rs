//! Cross-language features: definition, references, hover, and rename.

pub mod definition;
pub mod hover;
pub mod references;
pub mod rename;

use std::path::{Path, PathBuf};

use lsp_types::{Location, Position, Uri};
use typeweld_engine::ir::Span;

use crate::convert::{self, Encoding};
use crate::state::State;

/// The symbol under one cursor position, with the exact reference range.
pub struct Resolved {
    /// Index into `snapshot.packages`.
    pub package: usize,
    pub id: typeweld_engine::ir::SymbolId,
    /// Absolute path of the file the cursor is in.
    pub path: PathBuf,
    /// Byte range of the reference under the cursor.
    pub start: u32,
    pub end: u32,
}

impl Resolved {
    /// Whether the request originated in a Rust file.
    pub fn is_from_rust(&self) -> bool {
        has_extension(&self.path, &["rs"])
    }
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extensions.contains(&extension))
}

/// Resolves the symbol under `position`, looking through generated-file
/// marks, TypeScript usage refs, and Rust declaration spans.
pub fn resolve(state: &State, uri: &Uri, position: Position) -> Option<Resolved> {
    let path = convert::uri_to_path(uri)?;
    let text = state.read_text(&path)?;
    let offset = convert::offset_at(&text, position, state.encoding());
    let snapshot = state.snapshot()?;

    // Inside a generated package: the smallest covering mark wins.
    for (index, package) in snapshot.packages.iter().enumerate() {
        let Ok(relative) = path.strip_prefix(&package.package_dir) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        let mark = package
            .marks
            .iter()
            .filter(|mark| mark.file == relative && mark.start <= offset && offset <= mark.end)
            .min_by_key(|mark| mark.end - mark.start)?;
        return Some(Resolved {
            package: index,
            id: mark.symbol_id.clone(),
            path,
            start: mark.start,
            end: mark.end,
        });
    }

    if has_extension(&path, &["rs"]) {
        return resolve_rust(state, &text, &path, offset);
    }
    if has_extension(&path, &["ts", "tsx"]) {
        let key = convert::path_key(&path);
        let mut best: Option<Resolved> = None;
        for (index, package) in snapshot.packages.iter().enumerate() {
            if let Some(usage) = package.usage.symbol_at(&key, offset) {
                let better = best
                    .as_ref()
                    .is_none_or(|current| usage.end - usage.start < current.end - current.start);
                if better {
                    best = Some(Resolved {
                        package: index,
                        id: usage.symbol_id.clone(),
                        path: path.clone(),
                        start: usage.start,
                        end: usage.end,
                    });
                }
            }
        }
        return best;
    }
    None
}

/// Resolves a position in a Rust file: the smallest covering declaration name
/// span, or a router mount (which resolves to its endpoint).
fn resolve_rust(state: &State, text: &str, path: &Path, offset: u32) -> Option<Resolved> {
    let relative = convert::root_relative(state.root(), path)?;
    let snapshot = state.snapshot()?;

    let mut best: Option<(u32, Resolved)> = None;
    let mut consider = |size: u32, candidate: Resolved| {
        if best.as_ref().is_none_or(|(current, _)| size < *current) {
            best = Some((size, candidate));
        }
    };

    for (index, package) in snapshot.packages.iter().enumerate() {
        for symbol in &package.symbols {
            if symbol.name_span.contains(&relative, offset) {
                let span = &symbol.name_span;
                consider(
                    span.end - span.start,
                    Resolved {
                        package: index,
                        id: symbol.id.clone(),
                        path: path.to_path_buf(),
                        start: span.start,
                        end: span.end,
                    },
                );
            }
        }
        for endpoint in &package.contract.endpoints {
            for mount in &endpoint.router_mounts {
                if mount.contains(&relative, offset) {
                    let (start, end) = trailing_ident(text, mount, &endpoint.rust_name)
                        .unwrap_or((mount.start, mount.end));
                    consider(
                        mount.end - mount.start,
                        Resolved {
                            package: index,
                            id: endpoint.id.clone(),
                            path: path.to_path_buf(),
                            start,
                            end,
                        },
                    );
                }
            }
        }
    }
    best.map(|(_, resolved)| resolved)
}

/// The byte range of the trailing `name` identifier inside a router mount
/// span (which covers e.g. `users::get_user`), when the text matches.
pub fn trailing_ident(text: &str, mount: &Span, name: &str) -> Option<(u32, u32)> {
    let length = u32::try_from(name.len()).ok()?;
    let start = mount.end.checked_sub(length)?;
    (text.get(start as usize..mount.end as usize)? == name).then_some((start, mount.end))
}

/// The semantic TypeScript locations of a field or param property: the
/// plugin (inside the user's tsserver) is asked at the generated anchor
/// property; locations are filtered to user `.ts`/`.tsx` files and returned
/// as byte ranges. The single source for both rename edits and references.
pub(crate) fn property_locations(
    state: &State,
    plugin: Option<&crate::plugin::PluginHub>,
    package: &crate::state::PackageSnapshot,
    symbol: &crate::state::RustSymbol,
) -> Vec<(PathBuf, u32, u32)> {
    use crate::state::SymbolInfo;
    use typeweld_engine::gen::MarkKind;

    let Some(plugin) = plugin else {
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
        .find(|mark| mark.kind == kind && mark.symbol_id == symbol.id)
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
            eprintln!("typeweld-ls: semantic TypeScript property lookup unavailable: {message}");
            return Vec::new();
        }
    };
    let Some(snapshot) = state.snapshot() else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    for location in locations {
        let path = PathBuf::from(&location.file);
        let generated = snapshot
            .packages
            .iter()
            .any(|package| path.starts_with(&package.package_dir));
        if generated || !has_extension(&path, &["ts", "tsx"]) {
            continue;
        }
        let Some(text) = state.read_text(&path) else {
            continue;
        };
        let start = convert::byte_offset_from_utf16(&text, location.start);
        let end = convert::byte_offset_from_utf16(&text, location.start + location.length);
        ranges.push((path, start, end));
    }
    ranges
}

fn plugin_timeout() -> std::time::Duration {
    std::env::var("TYPEWELD_PLUGIN_TIMEOUT_SECS")
        .ok()
        .and_then(|seconds| seconds.parse().ok())
        .map_or(
            std::time::Duration::from_secs(3),
            std::time::Duration::from_secs,
        )
}

/// Builds the snapshot pushed to the TypeScript plugin: every generated-file
/// mark with its Rust target and hover text, plus the generated directories,
/// all in UTF-16 code units (the plugin lives in tsserver's string world).
pub fn plugin_snapshot(state: &State) -> Option<serde_json::Value> {
    let snapshot = state.snapshot()?;
    let mut generated_dirs = Vec::new();
    let mut marks = Vec::new();
    for package in &snapshot.packages {
        generated_dirs.push(package.package_dir.to_string_lossy().into_owned());
        for mark in &package.marks {
            let file = package.package_dir.join(&mark.file);
            let Some(text) = state.read_text(&file) else {
                continue;
            };
            let Some(symbol) = package.symbol_by_id(&mark.symbol_id) else {
                continue;
            };
            let target_path = state.root().join(&symbol.name_span.file);
            let Some(target_text) = state.read_text(&target_path) else {
                continue;
            };
            let hover = hover::markdown_for_symbol(package, symbol);
            marks.push(serde_json::json!({
                "file": file.to_string_lossy(),
                "start": convert::utf16_offset(&text, mark.start),
                "end": convert::utf16_offset(&text, mark.end),
                "symbol": mark.symbol_id,
                "kind": format!("{:?}", mark.kind),
                "target": {
                    "file": target_path.to_string_lossy(),
                    "start": convert::utf16_offset(&target_text, symbol.name_span.start),
                    "end": convert::utf16_offset(&target_text, symbol.name_span.end),
                },
                "hover": hover,
            }));
        }
    }
    Some(serde_json::json!({
        "method": "snapshot",
        "generation": state.generation(),
        "generatedDirs": generated_dirs,
        "marks": marks,
    }))
}

/// Converts a workspace-root-relative span to an LSP location.
pub fn span_location(state: &State, span: &Span) -> Option<Location> {
    let path = state.root().join(&span.file);
    let text = state.read_text(&path)?;
    Some(location(
        &path,
        &text,
        span.start,
        span.end,
        state.encoding(),
    ))
}

/// Builds a location from an absolute path and a byte range.
pub fn location(path: &Path, text: &str, start: u32, end: u32, encoding: Encoding) -> Location {
    Location::new(
        convert::path_to_uri(path),
        convert::range(text, start, end, encoding),
    )
}
