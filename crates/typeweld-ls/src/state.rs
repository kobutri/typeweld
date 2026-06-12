//! Server state: editor overlays plus the rebuilt cross-language snapshot.
//!
//! Every mutation (open/change/close, watched-file events) only flips a dirty
//! bit; [`State::ensure_fresh`] rebuilds the snapshot — config, workspace,
//! extraction, generation, usage scan — before the next feature request.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use typeweld_engine::config::Config;
use typeweld_engine::diag::{Diagnostic, Severity};
use typeweld_engine::extract;
use typeweld_engine::gen::{self, Mark};
use typeweld_engine::ir::{Contract, Span, SymbolId, TypeShape};
use typeweld_engine::usage::{self, UsageIndex};
use typeweld_engine::vfs::{
    DiskFileProvider, FileProvider, MemoryFileProvider, OverlayFileProvider,
};
use typeweld_engine::workspace;

use crate::convert::Encoding;

/// The complete mutable state of one server session.
pub struct State {
    root: PathBuf,
    config: Config,
    encoding: Encoding,
    /// Unsaved editor buffers, keyed by absolute path.
    overlays: MemoryFileProvider,
    /// Versions of open documents, keyed by absolute path.
    open_docs: HashMap<PathBuf, i32>,
    snapshot: Option<Snapshot>,
    dirty: bool,
    /// Monotonic rebuild counter, used to version plugin snapshots.
    generation: u64,
}

/// One coherent rebuild of every configured package.
pub struct Snapshot {
    pub packages: Vec<PackageSnapshot>,
    /// Files that received diagnostics in this round, for stale clearing.
    diag_files: HashSet<PathBuf>,
}

/// Everything the features need to answer requests for one package.
pub struct PackageSnapshot {
    /// Generated package name (also the retention key across rebuilds).
    pub ts_package: String,
    /// Absolute directory the generated package is written to.
    pub package_dir: PathBuf,
    pub contract: Contract,
    pub marks: Vec<Mark>,
    pub usage: UsageIndex,
    pub diagnostics: Vec<Diagnostic>,
    /// Precomputed navigable Rust declaration spans.
    pub symbols: Vec<RustSymbol>,
}

impl PackageSnapshot {
    /// Looks up the Rust declaration symbol with the given id.
    pub fn symbol_by_id(&self, id: &SymbolId) -> Option<&RustSymbol> {
        self.symbols.iter().find(|symbol| &symbol.id == id)
    }
}

/// One navigable Rust declaration: its id, name span, and contract position.
pub struct RustSymbol {
    pub id: SymbolId,
    pub name_span: Span,
    pub info: SymbolInfo,
}

/// Where a [`RustSymbol`] lives inside its package contract.
#[derive(Clone, Copy, Debug)]
pub enum SymbolInfo {
    /// Index into `contract.endpoints`.
    Endpoint(usize),
    /// Index into `contract.types`.
    Type(usize),
    /// Index into `contract.errors`.
    Error(usize),
    Field {
        owner: FieldOwner,
        field: usize,
    },
    Variant {
        ty: usize,
        variant: usize,
    },
    ErrorVariant {
        error: usize,
        variant: usize,
    },
    Param {
        endpoint: usize,
        query: bool,
        param: usize,
    },
}

/// The declaration a [`SymbolInfo::Field`] belongs to.
#[derive(Clone, Copy, Debug)]
pub enum FieldOwner {
    Struct(usize),
    Enum { ty: usize, variant: usize },
    Error { error: usize, variant: usize },
}

impl State {
    pub fn new(root: PathBuf, encoding: Encoding) -> Self {
        Self {
            root,
            config: Config::default(),
            encoding,
            overlays: MemoryFileProvider::default(),
            open_docs: HashMap::new(),
            snapshot: None,
            dirty: true,
            generation: 0,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()
    }

    pub fn version_of(&self, path: &Path) -> Option<i32> {
        self.open_docs.get(path).copied()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The open editor documents with their overlay text and version, for
    /// replay into a respawned rust-analyzer.
    pub fn open_documents(&self) -> Vec<(PathBuf, String, i32)> {
        self.open_docs
            .iter()
            .filter_map(|(path, version)| {
                let text = self.overlays.read(path)?;
                Some((path.clone(), text.to_string(), *version))
            })
            .collect()
    }

    /// Current text of `path`: editor overlay first, then disk.
    pub fn read_text(&self, path: &Path) -> Option<Arc<str>> {
        self.overlays
            .read(path)
            .or_else(|| DiskFileProvider.read(path))
    }

    pub fn open(&mut self, path: PathBuf, text: String, version: i32) {
        self.overlays.insert(path.clone(), text);
        self.open_docs.insert(path, version);
        self.dirty = true;
    }

    pub fn change(&mut self, path: PathBuf, text: String, version: i32) {
        self.open(path, text, version);
    }

    pub fn close(&mut self, path: &Path) {
        self.overlays.remove(path);
        self.open_docs.remove(path);
        self.dirty = true;
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Rebuilds the snapshot if anything changed since the last round.
    ///
    /// Returns the diagnostics to publish, grouped by absolute file; an entry
    /// with no diagnostics clears a file that had some in the previous round.
    pub fn ensure_fresh(&mut self) -> Vec<(PathBuf, Vec<Diagnostic>)> {
        if !self.dirty && self.snapshot.is_some() {
            return Vec::new();
        }
        self.dirty = false;
        self.generation += 1;

        let previous_diag_files = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.diag_files.clone())
            .unwrap_or_default();
        let mut previous_packages = self
            .snapshot
            .take()
            .map(|snapshot| snapshot.packages)
            .unwrap_or_default();

        let mut global_diagnostics = Vec::new();
        let packages = self.rebuild(&mut global_diagnostics, &mut previous_packages);

        let mut by_file: BTreeMap<PathBuf, Vec<Diagnostic>> = BTreeMap::new();
        let package_diagnostics = packages.iter().flat_map(|package| &package.diagnostics);
        for diagnostic in global_diagnostics.iter().chain(package_diagnostics) {
            let path = diagnostic.span.as_ref().map_or_else(
                || self.root.join("typeweld.toml"),
                |span| self.root.join(&span.file),
            );
            by_file.entry(path).or_default().push(diagnostic.clone());
        }

        let diag_files: HashSet<PathBuf> = by_file.keys().cloned().collect();
        let mut publishes: Vec<(PathBuf, Vec<Diagnostic>)> = by_file.into_iter().collect();
        for stale in previous_diag_files.difference(&diag_files) {
            publishes.push((stale.clone(), Vec::new()));
        }

        self.snapshot = Some(Snapshot {
            packages,
            diag_files,
        });
        publishes
    }

    /// Rebuilds every configured package. When a package fails to extract
    /// (e.g. a mid-edit syntax error), its previous snapshot is retained so
    /// navigation keeps working and the generated files on disk stay usable;
    /// only the diagnostics reflect the broken state.
    fn rebuild(
        &mut self,
        diagnostics: &mut Vec<Diagnostic>,
        previous_packages: &mut Vec<PackageSnapshot>,
    ) -> Vec<PackageSnapshot> {
        let disk = DiskFileProvider;
        let files = OverlayFileProvider {
            base: &disk,
            overlay: &self.overlays,
        };

        match Config::load(&self.root, &files) {
            Ok(config) => self.config = config,
            Err(diagnostic) => {
                diagnostics.push(diagnostic);
                return Vec::new();
            }
        }
        let (workspace, workspace_diagnostics) = match workspace::discover(&self.root, &files) {
            Ok(result) => result,
            Err(diagnostic) => {
                diagnostics.push(diagnostic);
                return Vec::new();
            }
        };
        diagnostics.extend(workspace_diagnostics);

        let package_dirs: Vec<PathBuf> = self
            .config
            .packages
            .iter()
            .map(|package| self.root.join(self.config.package_dir(&package.ts)))
            .collect();
        let ts_sources = collect_ts_sources(
            &self.root,
            &self.config,
            &self.overlays,
            &self.open_docs,
            &package_dirs,
        );

        let mut snapshots = Vec::new();
        for (package, package_dir) in self.config.packages.iter().zip(&package_dirs) {
            // Extraction parses with proc-macro2 span locations, which leak a
            // thread-local source map; a short-lived worker thread reclaims
            // that memory between rounds.
            let extraction = std::thread::scope(|scope| {
                scope
                    .spawn(|| extract::extract(&workspace, &package.cargo, &package.ts, &files))
                    .join()
            });
            let extraction = match extraction {
                Ok(Ok(extraction)) => extraction,
                Ok(Err(diagnostic)) => {
                    diagnostics.push(diagnostic);
                    if let Some(index) = previous_packages
                        .iter()
                        .position(|previous| previous.ts_package == package.ts)
                    {
                        snapshots.push(previous_packages.swap_remove(index));
                    }
                    continue;
                }
                Err(panic) => std::panic::resume_unwind(panic),
            };

            let options = gen::GenOptions {
                package_dir: Some(self.config.package_dir(&package.ts)),
                app_src: self.config.app_src.clone(),
            };
            let (generated, generation_diagnostics) = gen::generate(&extraction.contract, &options);
            let mut package_diagnostics = extraction.diagnostics;
            let failed = has_errors(&package_diagnostics) || has_errors(&generation_diagnostics);
            package_diagnostics.extend(generation_diagnostics);
            if failed {
                // Keep the previous generated files AND the previous
                // snapshot: a broken transient buffer must not replace the
                // last usable contract.
                if let Some(index) = previous_packages
                    .iter()
                    .position(|previous| previous.ts_package == package.ts)
                {
                    let mut previous = previous_packages.swap_remove(index);
                    previous.diagnostics = package_diagnostics;
                    snapshots.push(previous);
                    continue;
                }
            } else if let Err(message) = gen::write_package(package_dir, &generated) {
                package_diagnostics.push(Diagnostic::error("write-failed", message, None));
            }

            let usage = usage::scan_files(&extraction.contract, &ts_sources);
            let symbols = collect_symbols(&extraction.contract);
            snapshots.push(PackageSnapshot {
                ts_package: package.ts.clone(),
                package_dir: package_dir.clone(),
                contract: extraction.contract,
                marks: generated.marks,
                usage,
                diagnostics: package_diagnostics,
                symbols,
            });
        }
        snapshots
    }
}

fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
}

/// Gathers the TS sources to scan for usage: the configured `app.src` globs
/// plus any open TS document under the root, excluding generated packages.
fn collect_ts_sources(
    root: &Path,
    config: &Config,
    overlays: &MemoryFileProvider,
    open_docs: &HashMap<PathBuf, i32>,
    package_dirs: &[PathBuf],
) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    let mut sources = Vec::new();
    let mut push = |path: PathBuf, sources: &mut Vec<(String, String)>| {
        if package_dirs.iter().any(|dir| path.starts_with(dir)) || !seen.insert(path.clone()) {
            return;
        }
        let Some(contents) = overlays
            .read(&path)
            .or_else(|| DiskFileProvider.read(&path))
        else {
            return;
        };
        sources.push((crate::convert::path_key(&path), contents.to_string()));
    };

    for pattern in &config.app_src {
        let pattern = root.join(pattern).to_string_lossy().into_owned();
        let Ok(paths) = glob::glob(&pattern) else {
            continue;
        };
        for path in paths.flatten() {
            push(path, &mut sources);
        }
    }
    for path in open_docs.keys() {
        let is_ts = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension == "ts" || extension == "tsx");
        if is_ts && path.starts_with(root) {
            push(path.clone(), &mut sources);
        }
    }
    sources
}

/// Flattens the contract into navigable Rust declaration spans.
fn collect_symbols(contract: &Contract) -> Vec<RustSymbol> {
    let mut symbols = Vec::new();

    for (index, endpoint) in contract.endpoints.iter().enumerate() {
        symbols.push(RustSymbol {
            id: endpoint.id.clone(),
            name_span: endpoint.spans.name.clone(),
            info: SymbolInfo::Endpoint(index),
        });
        for (param, declaration) in endpoint.request.path_params.iter().enumerate() {
            symbols.push(RustSymbol {
                id: declaration.id.clone(),
                name_span: declaration.span.clone(),
                info: SymbolInfo::Param {
                    endpoint: index,
                    query: false,
                    param,
                },
            });
        }
        for (param, declaration) in endpoint.request.query_params.iter().enumerate() {
            symbols.push(RustSymbol {
                id: declaration.id.clone(),
                name_span: declaration.span.clone(),
                info: SymbolInfo::Param {
                    endpoint: index,
                    query: true,
                    param,
                },
            });
        }
    }

    for (ty, decl) in contract.types.iter().enumerate() {
        symbols.push(RustSymbol {
            id: decl.id.clone(),
            name_span: decl.spans.name.clone(),
            info: SymbolInfo::Type(ty),
        });
        match &decl.shape {
            TypeShape::Struct { fields } => {
                for (field, declaration) in fields.iter().enumerate() {
                    symbols.push(RustSymbol {
                        id: declaration.id.clone(),
                        name_span: declaration.spans.name.clone(),
                        info: SymbolInfo::Field {
                            owner: FieldOwner::Struct(ty),
                            field,
                        },
                    });
                }
            }
            TypeShape::Enum { variants } => {
                for (variant, declaration) in variants.iter().enumerate() {
                    symbols.push(RustSymbol {
                        id: declaration.id.clone(),
                        name_span: declaration.spans.name.clone(),
                        info: SymbolInfo::Variant { ty, variant },
                    });
                    for (field, field_declaration) in declaration.fields.iter().enumerate() {
                        symbols.push(RustSymbol {
                            id: field_declaration.id.clone(),
                            name_span: field_declaration.spans.name.clone(),
                            info: SymbolInfo::Field {
                                owner: FieldOwner::Enum { ty, variant },
                                field,
                            },
                        });
                    }
                }
            }
            TypeShape::Newtype(_) => {}
        }
    }

    for (error, decl) in contract.errors.iter().enumerate() {
        symbols.push(RustSymbol {
            id: decl.id.clone(),
            name_span: decl.spans.name.clone(),
            info: SymbolInfo::Error(error),
        });
        for (variant, declaration) in decl.variants.iter().enumerate() {
            symbols.push(RustSymbol {
                id: declaration.id.clone(),
                name_span: declaration.spans.name.clone(),
                info: SymbolInfo::ErrorVariant { error, variant },
            });
            for (field, field_declaration) in declaration.fields.iter().enumerate() {
                symbols.push(RustSymbol {
                    id: field_declaration.id.clone(),
                    name_span: field_declaration.spans.name.clone(),
                    info: SymbolInfo::Field {
                        owner: FieldOwner::Error { error, variant },
                        field,
                    },
                });
            }
        }
    }

    symbols
}
