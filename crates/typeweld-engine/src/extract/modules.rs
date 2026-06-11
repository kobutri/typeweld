//! Module-tree walking and per-module symbol tables.
//!
//! Each workspace crate is walked from its crate root, following `mod x;`
//! declarations (including `#[path]` overrides) and inline modules. The result
//! is a [`ModuleMap`]: for every module, its items plus the `use` imports
//! needed for structural name resolution.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::diag::Diagnostic;
use crate::ir::Span;
use crate::vfs::FileProvider;
use crate::workspace::Package;

/// All modules of all walked crates, keyed by module path
/// (`[crate_ident, submodule, ...]`).
#[derive(Default)]
pub struct ModuleMap {
    pub modules: HashMap<Vec<String>, ModuleData>,
}

/// One module's items and import table.
pub struct ModuleData {
    /// Workspace-root-relative path of the file containing this module.
    pub file: String,
    pub items: Vec<syn::Item>,
    /// Child module name to module key.
    pub mods: HashMap<String, Vec<String>>,
    /// Imported name to the path it was imported from, as written.
    pub uses: HashMap<String, Vec<String>>,
    /// Glob-imported module paths, as written.
    pub globs: Vec<Vec<String>>,
}

impl ModuleMap {
    /// Walks `package`'s crate and adds its modules to the map.
    pub fn walk_package(
        &mut self,
        workspace_root: &Path,
        package: &Package,
        files: &dyn FileProvider,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let crate_key = vec![package.ident.clone()];
        if self.modules.contains_key(&crate_key) {
            return;
        }
        self.walk_file(
            workspace_root,
            &package.crate_root,
            crate_key,
            files,
            diagnostics,
        );
    }

    fn walk_file(
        &mut self,
        workspace_root: &Path,
        file: &Path,
        module_key: Vec<String>,
        files: &dyn FileProvider,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let rel_file = relative_path(workspace_root, file);
        let Some(source) = files.read(file) else {
            diagnostics.push(Diagnostic::error(
                "missing-module-file",
                format!("module file not found: {rel_file}"),
                None,
            ));
            return;
        };
        let parsed = match syn::parse_file(&source) {
            Ok(parsed) => parsed,
            Err(error) => {
                let span = error.span();
                let range = span.byte_range();
                diagnostics.push(Diagnostic::error(
                    "parse-error",
                    format!("failed to parse {rel_file}: {error}"),
                    Some(Span {
                        file: rel_file,
                        start: u32::try_from(range.start).unwrap_or(0),
                        end: u32::try_from(range.end).unwrap_or(0),
                    }),
                ));
                return;
            }
        };

        self.add_module(
            workspace_root,
            file,
            &rel_file,
            module_key,
            parsed.items,
            files,
            diagnostics,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn add_module(
        &mut self,
        workspace_root: &Path,
        file: &Path,
        rel_file: &str,
        module_key: Vec<String>,
        items: Vec<syn::Item>,
        files: &dyn FileProvider,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let mut data = ModuleData {
            file: rel_file.to_owned(),
            items: Vec::new(),
            mods: HashMap::new(),
            uses: HashMap::new(),
            globs: Vec::new(),
        };

        for item in items {
            match item {
                syn::Item::Mod(module) if is_cfg_test(&module.attrs) => {}
                syn::Item::Mod(module) => {
                    let name = module.ident.to_string();
                    let mut child_key = module_key.clone();
                    child_key.push(name.clone());
                    data.mods.insert(name.clone(), child_key.clone());

                    if let Some((_, child_items)) = module.content {
                        self.add_module(
                            workspace_root,
                            file,
                            rel_file,
                            child_key,
                            child_items,
                            files,
                            diagnostics,
                        );
                    } else {
                        match module_file_candidates(file, &module_key, &name, &module.attrs)
                            .into_iter()
                            .find(|candidate| files.exists(candidate))
                        {
                            Some(child_file) => self.walk_file(
                                workspace_root,
                                &child_file,
                                child_key,
                                files,
                                diagnostics,
                            ),
                            None => diagnostics.push(Diagnostic::warning(
                                "missing-module-file",
                                format!("file for module `{name}` not found (from {rel_file})"),
                                None,
                            )),
                        }
                    }
                }
                syn::Item::Use(use_item) if !is_cfg_test(&use_item.attrs) => {
                    collect_use_tree(&use_item.tree, &mut Vec::new(), &mut data);
                    data.items.push(syn::Item::Use(use_item));
                }
                item if is_cfg_test(item_attrs(&item)) => {}
                item => data.items.push(item),
            }
        }

        self.modules.insert(module_key, data);
    }
}

fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Struct(item) => &item.attrs,
        syn::Item::Enum(item) => &item.attrs,
        syn::Item::Fn(item) => &item.attrs,
        syn::Item::Type(item) => &item.attrs,
        syn::Item::Use(item) => &item.attrs,
        syn::Item::Const(item) => &item.attrs,
        syn::Item::Static(item) => &item.attrs,
        syn::Item::Trait(item) => &item.attrs,
        syn::Item::Impl(item) => &item.attrs,
        _ => &[],
    }
}

fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("cfg") {
            return false;
        }
        let mut is_test = false;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("test") {
                is_test = true;
            }
            // Consume any value to keep the parser happy.
            if meta.input.peek(syn::Token![=]) {
                let _: syn::Lit = meta.value()?.parse()?;
            }
            Ok(())
        });
        is_test
    })
}

/// Candidate files for `mod name;` declared in `file`.
fn module_file_candidates(
    file: &Path,
    module_key: &[String],
    name: &str,
    attrs: &[syn::Attribute],
) -> Vec<PathBuf> {
    let dir = file.parent().unwrap_or_else(|| Path::new(""));

    // `#[path = "..."]` wins outright.
    for attr in attrs {
        if attr.path().is_ident("path") {
            if let syn::Meta::NameValue(meta) = &attr.meta {
                if let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(value),
                    ..
                }) = &meta.value
                {
                    return vec![dir.join(value.value())];
                }
            }
        }
    }

    let file_stem = file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    let is_crate_root = module_key.len() == 1;
    let is_mod_rs = file_stem == "mod";

    if is_crate_root || is_mod_rs {
        vec![
            dir.join(format!("{name}.rs")),
            dir.join(name).join("mod.rs"),
        ]
    } else {
        // 2018-style: submodules of `foo.rs` live in `foo/`.
        vec![
            dir.join(file_stem).join(format!("{name}.rs")),
            dir.join(file_stem).join(name).join("mod.rs"),
        ]
    }
}

fn collect_use_tree(tree: &syn::UseTree, prefix: &mut Vec<String>, data: &mut ModuleData) {
    match tree {
        syn::UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_use_tree(&path.tree, prefix, data);
            prefix.pop();
        }
        syn::UseTree::Name(name) => {
            let ident = name.ident.to_string();
            let mut path = prefix.clone();
            if ident == "self" {
                // `use a::b::{self}` imports `b`.
                if let Some(last) = path.last().cloned() {
                    data.uses.insert(last, path);
                }
            } else {
                path.push(ident.clone());
                data.uses.insert(ident, path);
            }
        }
        syn::UseTree::Rename(rename) => {
            let mut path = prefix.clone();
            path.push(rename.ident.to_string());
            data.uses.insert(rename.rename.to_string(), path);
        }
        syn::UseTree::Glob(_) => {
            data.globs.push(prefix.clone());
        }
        syn::UseTree::Group(group) => {
            for tree in &group.items {
                collect_use_tree(tree, prefix, data);
            }
        }
    }
}

/// Workspace-root-relative path with `/` separators.
pub fn relative_path(workspace_root: &Path, file: &Path) -> String {
    file.strip_prefix(workspace_root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Builds an IR span from a syn spanned item within `rel_file`.
pub fn span_of(rel_file: &str, spanned: &dyn quote_span::Spanned) -> Span {
    let range = spanned.span_range();
    Span {
        file: rel_file.to_owned(),
        start: u32::try_from(range.start).unwrap_or(0),
        end: u32::try_from(range.end).unwrap_or(0),
    }
}

/// Object-safe byte-range access for syn nodes.
pub mod quote_span {
    pub trait Spanned {
        fn span_range(&self) -> std::ops::Range<usize>;
    }

    impl<T: syn::spanned::Spanned> Spanned for T {
        fn span_range(&self) -> std::ops::Range<usize> {
            self.span().byte_range()
        }
    }
}
