//! Structural name resolution.
//!
//! Resolves the type paths appearing in API positions (fields, params,
//! response bodies) to workspace declarations, builtin wrappers, primitives,
//! or known external types — without compiling anything. Resolution is
//! deliberately structural: anything it cannot see produces a diagnostic
//! rather than a guess.

use crate::ir::ExternalType;

use super::modules::ModuleMap;

/// Resolution result for a single name or path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Entity {
    /// A module in the walked workspace.
    Module(Vec<String>),
    /// An item declared in a walked module: `(module key, item name)`.
    Item(Vec<String>, String),
    /// A known external type with a fixed wire mapping.
    External(ExternalType),
    /// An external crate root that may contain known external types.
    ExternalCrate(String),
    Unknown,
}

/// Resolver over a walked [`ModuleMap`].
pub struct Resolver<'a> {
    pub modules: &'a ModuleMap,
    /// Cargo idents of all walked crates.
    pub crate_idents: Vec<String>,
}

const MAX_RESOLUTION_DEPTH: usize = 32;

impl Resolver<'_> {
    /// Resolves a multi-segment path written in `module`.
    #[must_use]
    pub fn resolve_path(&self, module: &[String], segments: &[String]) -> Entity {
        self.resolve_path_inner(module, segments, 0)
    }

    fn resolve_path_inner(&self, module: &[String], segments: &[String], depth: usize) -> Entity {
        if depth > MAX_RESOLUTION_DEPTH || segments.is_empty() {
            return Entity::Unknown;
        }

        let first = segments[0].as_str();
        let rest = &segments[1..];

        // Leading path qualifiers.
        match first {
            "crate" => {
                let crate_root = vec![module[0].clone()];
                return self.resolve_from_module(&crate_root, rest, depth + 1);
            }
            "self" => return self.resolve_from_module(module, rest, depth + 1),
            "super" => {
                if module.len() < 2 {
                    return Entity::Unknown;
                }
                let parent = module[..module.len() - 1].to_vec();
                return self.resolve_path_inner(
                    &parent,
                    &normalized_super_rest(segments),
                    depth + 1,
                );
            }
            _ => {}
        }

        if let Some(external) = known_external(segments) {
            return external;
        }

        // A walked workspace crate referenced by ident.
        if rest.is_empty() {
            return self.resolve_name(module, first, depth + 1);
        }
        if self.crate_idents.iter().any(|ident| ident == first) {
            return self.resolve_from_module(&[first.to_owned()], rest, depth + 1);
        }

        // Otherwise the first segment must be a locally visible name
        // (module, import alias, or re-export).
        match self.resolve_name(module, first, depth + 1) {
            Entity::Module(target) => self.resolve_from_module(&target, rest, depth + 1),
            Entity::ExternalCrate(krate) => {
                let mut full = vec![krate];
                full.extend(rest.iter().cloned());
                known_external(&full).unwrap_or(Entity::Unknown)
            }
            _ => Entity::Unknown,
        }
    }

    /// Resolves segments starting from a known module.
    fn resolve_from_module(&self, module: &[String], segments: &[String], depth: usize) -> Entity {
        if depth > MAX_RESOLUTION_DEPTH {
            return Entity::Unknown;
        }
        if segments.is_empty() {
            return Entity::Module(module.to_vec());
        }

        let first = segments[0].as_str();
        let rest = &segments[1..];

        if first == "super" {
            if module.len() < 2 {
                return Entity::Unknown;
            }
            return self.resolve_from_module(&module[..module.len() - 1], rest, depth + 1);
        }
        if first == "self" {
            return self.resolve_from_module(module, rest, depth + 1);
        }

        match self.resolve_name(module, first, depth + 1) {
            Entity::Module(target) => self.resolve_from_module(&target, rest, depth + 1),
            Entity::Item(module, name) if rest.is_empty() => Entity::Item(module, name),
            Entity::External(external) if rest.is_empty() => Entity::External(external),
            Entity::ExternalCrate(krate) => {
                let mut full = vec![krate];
                full.extend(rest.iter().cloned());
                known_external(&full).unwrap_or(Entity::Unknown)
            }
            _ => Entity::Unknown,
        }
    }

    /// Resolves a single name visible inside `module`.
    #[must_use]
    pub fn resolve_name(&self, module: &[String], name: &str, depth: usize) -> Entity {
        if depth > MAX_RESOLUTION_DEPTH {
            return Entity::Unknown;
        }
        let Some(data) = self.modules.modules.get(module) else {
            return Entity::Unknown;
        };

        // Local declarations win.
        if data.mods.contains_key(name) {
            return Entity::Module(data.mods[name].clone());
        }
        if module_declares_item(data, name) {
            return Entity::Item(module.to_vec(), name.to_owned());
        }

        // Imports (including renames and re-export chains).
        if let Some(path) = data.uses.get(name) {
            return self.resolve_path_inner(module, path, depth + 1);
        }

        // Glob imports of walked modules.
        for glob in &data.globs {
            let target = match self.resolve_path_inner(module, glob, depth + 1) {
                Entity::Module(target) => target,
                _ => continue,
            };
            let resolved = self.resolve_name(&target, name, depth + 1);
            if resolved != Entity::Unknown {
                return resolved;
            }
        }

        // A bare external crate name (usable as path root via alias).
        if matches!(name, "uuid" | "chrono" | "rust_decimal" | "serde_json") {
            return Entity::ExternalCrate(name.to_owned());
        }
        if self.crate_idents.iter().any(|ident| ident == name) {
            return Entity::Module(vec![name.to_owned()]);
        }

        Entity::Unknown
    }
}

fn module_declares_item(data: &super::modules::ModuleData, name: &str) -> bool {
    data.items.iter().any(|item| match item {
        syn::Item::Struct(item) => item.ident == name,
        syn::Item::Enum(item) => item.ident == name,
        syn::Item::Type(item) => item.ident == name,
        syn::Item::Fn(item) => item.sig.ident == name,
        _ => false,
    })
}

/// `super::super::x` handling: drops exactly one leading `super`.
fn normalized_super_rest(segments: &[String]) -> Vec<String> {
    segments[1..].to_vec()
}

/// Matches fully spelled external type paths.
///
/// `chrono::DateTime` resolves here, but type lowering additionally checks
/// that its generic argument is `Utc` before accepting it.
fn known_external(segments: &[String]) -> Option<Entity> {
    let parts: Vec<&str> = segments.iter().map(String::as_str).collect();
    match parts.as_slice() {
        ["uuid", "Uuid"] => Some(Entity::External(ExternalType::Uuid)),
        ["rust_decimal", "Decimal"] => Some(Entity::External(ExternalType::Decimal)),
        ["serde_json", "Value"] => Some(Entity::External(ExternalType::JsonValue)),
        ["chrono", "DateTime"] => Some(Entity::External(ExternalType::DateTimeUtc)),
        _ => None,
    }
}
