//! TypeScript usage analysis for generated bindings (oxc-based).
//!
//! Scans user TypeScript sources for references to the symbols of one
//! generated API package and builds a [`UsageIndex`]. The index serves two
//! consumers:
//!
//! - the CLI unused-endpoint lint, via [`UsageIndex::strongly_used_endpoints`]
//! - the language server, via [`UsageIndex::refs_for`] and
//!   [`UsageIndex::symbol_at`] (find-references, goto-definition, rename)
//!
//! Resolution is purely syntactic: import declarations whose specifier is the
//! contract's package (or one of its known subpaths) introduce bindings, and
//! identifier references and static member chains are resolved against those
//! bindings. Computed access (`api["getUser"]`) is intentionally out of scope.

use std::collections::{HashMap, HashSet};

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, CallExpression, ExportNamedDeclaration, Expression, IdentifierReference,
    ImportDeclaration, ImportDeclarationSpecifier, NewExpression, ObjectPropertyKind, Program,
    PropertyKey, ReturnStatement, Statement, StaticMemberExpression, TSType, TSTypeAnnotation,
    VariableDeclarator, YieldExpression,
};
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use serde::{Deserialize, Serialize};

use crate::ir::{Contract, SymbolId};

/// How confidently a reference counts as a real usage.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum UsageStrength {
    /// The symbol is invoked or passed somewhere; the endpoint is in use.
    Strong,
    /// The symbol is only mentioned (import, re-export, type position).
    Weak,
}

/// What kind of generated symbol a reference points at.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum UsageKind {
    /// An endpoint accessor, e.g. `getUser`.
    Endpoint,
    /// An endpoint route metadata const, e.g. `getUserRoute`.
    Route,
    /// A schema type/const, e.g. `User`.
    Type,
    /// An error union, e.g. `UserError`.
    Error,
    /// An error variant class, e.g. `UserNotFound`.
    ErrorVariant,
    /// An error variant tag string inside `catchTag`/`catchTags`.
    ErrorTag,
}

/// One reference to a generated API symbol in user TypeScript code.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsageRef {
    pub symbol_id: SymbolId,
    pub kind: UsageKind,
    pub strength: UsageStrength,
    /// File path as passed in by the caller.
    pub file: String,
    /// Byte offset of the start of the reference.
    pub start: u32,
    /// Byte offset of the end of the reference.
    pub end: u32,
}

/// Usage index over a set of TS source files for one contract.
#[derive(Debug, Default)]
pub struct UsageIndex {
    /// Every recorded reference, in scan order.
    pub refs: Vec<UsageRef>,
    by_symbol: HashMap<SymbolId, Vec<usize>>,
}

impl UsageIndex {
    fn push(&mut self, usage: UsageRef) {
        self.by_symbol
            .entry(usage.symbol_id.clone())
            .or_default()
            .push(self.refs.len());
        self.refs.push(usage);
    }

    /// All refs to one symbol.
    #[must_use]
    pub fn refs_for(&self, id: &SymbolId) -> Vec<&UsageRef> {
        self.by_symbol
            .get(id)
            .map(|indices| indices.iter().map(|&index| &self.refs[index]).collect())
            .unwrap_or_default()
    }

    /// The symbol whose ref covers the byte `offset` in `file`, if any.
    ///
    /// When refs nest (a tag literal inside a call argument), the narrowest
    /// covering ref wins.
    #[must_use]
    pub fn symbol_at(&self, file: &str, offset: u32) -> Option<&UsageRef> {
        self.refs
            .iter()
            .filter(|usage| usage.file == file && offset >= usage.start && offset <= usage.end)
            .min_by_key(|usage| usage.end - usage.start)
    }

    /// Endpoint ids that have at least one [`UsageStrength::Strong`] usage.
    ///
    /// Route refs carry their endpoint's id, so referencing route metadata
    /// counts the endpoint as used.
    #[must_use]
    pub fn strongly_used_endpoints(&self) -> HashSet<SymbolId> {
        self.refs
            .iter()
            .filter(|usage| {
                usage.strength == UsageStrength::Strong
                    && matches!(usage.kind, UsageKind::Endpoint | UsageKind::Route)
            })
            .map(|usage| usage.symbol_id.clone())
            .collect()
    }
}

/// Scans one TS/TSX source file for references to `contract` symbols.
///
/// `file` is recorded verbatim in the produced refs and also used to pick the
/// source type from its extension (falling back to plain TypeScript).
pub fn scan_file(contract: &Contract, file: &str, source: &str, index: &mut UsageIndex) {
    let table = SymbolTable::new(contract);
    scan_with_table(&table, file, source, index);
}

/// Scans `(file, source)` pairs and returns the combined index.
#[must_use]
pub fn scan_files(contract: &Contract, files: &[(String, String)]) -> UsageIndex {
    let table = SymbolTable::new(contract);
    let mut index = UsageIndex::default();
    for (file, source) in files {
        scan_with_table(&table, file, source, &mut index);
    }
    index
}

fn scan_with_table(table: &SymbolTable, file: &str, source: &str, index: &mut UsageIndex) {
    let source_type = SourceType::from_path(file).unwrap_or_else(|_| SourceType::ts());
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();

    let mut scanner = Scanner {
        table,
        file,
        index,
        bindings: HashMap::new(),
        type_depth: 0,
        strong_depth: 0,
    };
    scanner.collect_module_records(&parsed.program);
    scanner.visit_program(&parsed.program);
}

/// A resolvable name with the symbol it denotes.
#[derive(Clone, Debug)]
struct NameTarget {
    id: SymbolId,
    kind: UsageKind,
}

/// One generated module a specifier or namespace can point at.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Scope {
    /// The package root (`index.ts`): everything re-exported.
    Root,
    /// One `endpoints/<module>.ts` file.
    Module(String),
    /// `schemas.ts`.
    Schemas,
    /// `errors.ts`.
    Errors,
}

/// What a local import binding refers to.
#[derive(Clone, Debug)]
enum Binding {
    /// A concrete generated symbol.
    Symbol(NameTarget),
    /// A namespace object over one generated module.
    Namespace(Scope),
}

/// Name lookup tables derived from one [`Contract`].
#[derive(Debug)]
struct SymbolTable {
    package: String,
    root: HashMap<String, NameTarget>,
    modules: HashMap<String, HashMap<String, NameTarget>>,
    schemas: HashMap<String, NameTarget>,
    errors: HashMap<String, NameTarget>,
    /// Error variant `_tag` value -> variant symbol.
    tags: HashMap<String, SymbolId>,
}

impl SymbolTable {
    fn new(contract: &Contract) -> Self {
        let mut root = HashMap::new();
        let mut modules: HashMap<String, HashMap<String, NameTarget>> = HashMap::new();
        let mut schemas = HashMap::new();
        let mut errors = HashMap::new();
        let mut tags = HashMap::new();

        for endpoint in &contract.endpoints {
            let accessor = NameTarget {
                id: endpoint.id.clone(),
                kind: UsageKind::Endpoint,
            };
            let route = NameTarget {
                id: endpoint.id.clone(),
                kind: UsageKind::Route,
            };
            let route_name = format!("{}Route", endpoint.ts_name);
            let module = modules.entry(endpoint.ts_module.clone()).or_default();
            module.insert(endpoint.ts_name.clone(), accessor.clone());
            module.insert(route_name.clone(), route.clone());
            root.insert(endpoint.ts_name.clone(), accessor);
            root.insert(route_name, route);
        }

        for decl in &contract.types {
            let target = NameTarget {
                id: decl.id.clone(),
                kind: UsageKind::Type,
            };
            schemas.insert(decl.ts_name.clone(), target.clone());
            root.insert(decl.ts_name.clone(), target);
        }

        for decl in &contract.errors {
            let union = NameTarget {
                id: decl.id.clone(),
                kind: UsageKind::Error,
            };
            errors.insert(decl.ts_name.clone(), union.clone());
            root.insert(decl.ts_name.clone(), union);
            for variant in &decl.variants {
                let target = NameTarget {
                    id: variant.id.clone(),
                    kind: UsageKind::ErrorVariant,
                };
                errors.insert(variant.tag.clone(), target.clone());
                root.insert(variant.tag.clone(), target);
                tags.insert(variant.tag.clone(), variant.id.clone());
            }
        }

        Self {
            package: contract.package.ts_package.clone(),
            root,
            modules,
            schemas,
            errors,
            tags,
        }
    }

    /// Maps an import specifier string to the generated module it names.
    fn scope_for_specifier(&self, specifier: &str) -> Option<Scope> {
        if specifier == self.package {
            return Some(Scope::Root);
        }
        let subpath = specifier
            .strip_prefix(self.package.as_str())?
            .strip_prefix('/')?;
        match subpath {
            "schemas" => Some(Scope::Schemas),
            "errors" => Some(Scope::Errors),
            _ => {
                let module = subpath.strip_prefix("endpoints/")?;
                self.modules
                    .contains_key(module)
                    .then(|| Scope::Module(module.to_owned()))
            }
        }
    }

    /// Looks up an exported symbol name within one generated module.
    fn lookup(&self, scope: &Scope, name: &str) -> Option<&NameTarget> {
        match scope {
            Scope::Root => self.root.get(name),
            Scope::Module(module) => self.modules.get(module)?.get(name),
            Scope::Schemas => self.schemas.get(name),
            Scope::Errors => self.errors.get(name),
        }
    }

    /// Resolves a named import from one generated module to a binding.
    ///
    /// `index.ts` does `export * as <module> from "./endpoints/<module>"`, so
    /// endpoint module names are importable from the root as namespaces.
    fn resolve_named_import(&self, scope: &Scope, name: &str) -> Option<Binding> {
        if let Some(target) = self.lookup(scope, name) {
            return Some(Binding::Symbol(target.clone()));
        }
        if *scope == Scope::Root && self.modules.contains_key(name) {
            return Some(Binding::Namespace(Scope::Module(name.to_owned())));
        }
        None
    }
}

struct Scanner<'t, 'i> {
    table: &'t SymbolTable,
    file: &'t str,
    index: &'i mut UsageIndex,
    bindings: HashMap<String, Binding>,
    /// Depth of enclosing TypeScript type positions.
    type_depth: u32,
    /// Depth of enclosing call/new/return/yield/initializer positions.
    strong_depth: u32,
}

impl Scanner<'_, '_> {
    /// Collects import bindings and re-export refs before walking the body,
    /// so that references textually before their import still resolve.
    fn collect_module_records(&mut self, program: &Program<'_>) {
        for statement in &program.body {
            match statement {
                Statement::ImportDeclaration(decl) => self.collect_import(decl),
                Statement::ExportNamedDeclaration(decl) => self.collect_reexport(decl),
                _ => {}
            }
        }
    }

    fn collect_import(&mut self, decl: &ImportDeclaration<'_>) {
        let Some(scope) = self.table.scope_for_specifier(decl.source.value.as_str()) else {
            return;
        };
        let Some(specifiers) = &decl.specifiers else {
            return;
        };
        for specifier in specifiers {
            match specifier {
                ImportDeclarationSpecifier::ImportSpecifier(named) => {
                    let imported = named.imported.name();
                    let Some(binding) = self.table.resolve_named_import(&scope, &imported) else {
                        continue;
                    };
                    // The import itself only mentions the symbol: Weak.
                    if let Binding::Symbol(target) = &binding {
                        let target = target.clone();
                        self.push_ref(&target, named.imported.span(), UsageStrength::Weak);
                    }
                    self.bindings.insert(named.local.name.to_string(), binding);
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(namespace) => {
                    self.bindings.insert(
                        namespace.local.name.to_string(),
                        Binding::Namespace(scope.clone()),
                    );
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(_) => {}
            }
        }
    }

    /// `export { getUser } from "<pkg>"` re-exports without using: Weak.
    fn collect_reexport(&mut self, decl: &ExportNamedDeclaration<'_>) {
        let Some(source) = &decl.source else { return };
        let Some(scope) = self.table.scope_for_specifier(source.value.as_str()) else {
            return;
        };
        for specifier in &decl.specifiers {
            let local = specifier.local.name();
            if let Some(target) = self.table.lookup(&scope, &local).cloned() {
                self.push_ref(&target, specifier.local.span(), UsageStrength::Weak);
            }
        }
    }

    /// Resolves an expression that denotes a namespace over a generated
    /// module: a namespace import binding, or `<rootNs>.<module>`.
    fn resolve_namespace(&self, expr: &Expression<'_>) -> Option<Scope> {
        match expr {
            Expression::Identifier(ident) => match self.bindings.get(ident.name.as_str()) {
                Some(Binding::Namespace(scope)) => Some(scope.clone()),
                _ => None,
            },
            Expression::StaticMemberExpression(member) => {
                let module = member.property.name.as_str();
                (self.resolve_namespace(&member.object)? == Scope::Root
                    && self.table.modules.contains_key(module))
                .then(|| Scope::Module(module.to_owned()))
            }
            _ => None,
        }
    }

    /// Records a value- or type-position reference with computed strength.
    fn record_reference(&mut self, target: &NameTarget, span: Span) {
        let strength = match target.kind {
            // Referencing route metadata means the endpoint is intentionally
            // used, regardless of position.
            UsageKind::Route | UsageKind::ErrorTag => UsageStrength::Strong,
            _ if self.type_depth > 0 => UsageStrength::Weak,
            _ if self.strong_depth > 0 => UsageStrength::Strong,
            _ => UsageStrength::Weak,
        };
        self.push_ref(target, span, strength);
    }

    fn push_ref(&mut self, target: &NameTarget, span: Span, strength: UsageStrength) {
        self.index.push(UsageRef {
            symbol_id: target.id.clone(),
            kind: target.kind,
            strength,
            file: self.file.to_owned(),
            start: span.start,
            end: span.end,
        });
    }

    /// Records `ErrorTag` usages for `catchTag("Tag", ...)` string-literal
    /// arguments and `catchTags({ Tag: ... })` object keys.
    fn scan_catch_tags(&mut self, call: &CallExpression<'_>) {
        let callee = match &call.callee {
            Expression::Identifier(ident) => ident.name.as_str(),
            Expression::StaticMemberExpression(member) => member.property.name.as_str(),
            _ => return,
        };
        if callee.ends_with("catchTag") {
            for argument in &call.arguments {
                if let Argument::StringLiteral(literal) = argument {
                    self.record_tag(literal.value.as_str(), literal.span);
                }
            }
        } else if callee.ends_with("catchTags") {
            for argument in &call.arguments {
                let Argument::ObjectExpression(object) = argument else {
                    continue;
                };
                for property in &object.properties {
                    let ObjectPropertyKind::ObjectProperty(property) = property else {
                        continue;
                    };
                    match &property.key {
                        PropertyKey::StaticIdentifier(key) => {
                            self.record_tag(key.name.as_str(), key.span);
                        }
                        PropertyKey::StringLiteral(key) => {
                            self.record_tag(key.value.as_str(), key.span);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn record_tag(&mut self, tag: &str, span: Span) {
        if let Some(id) = self.table.tags.get(tag) {
            let target = NameTarget {
                id: id.clone(),
                kind: UsageKind::ErrorTag,
            };
            self.push_ref(&target, span, UsageStrength::Strong);
        }
    }
}

impl<'a> Visit<'a> for Scanner<'_, '_> {
    fn visit_import_declaration(&mut self, _it: &ImportDeclaration<'a>) {
        // Handled in `collect_module_records`.
    }

    fn visit_export_named_declaration(&mut self, it: &ExportNamedDeclaration<'a>) {
        if it.source.is_some() {
            // Re-exports are handled in `collect_module_records`.
            return;
        }
        walk::walk_export_named_declaration(self, it);
    }

    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if let Some(Binding::Symbol(target)) = self.bindings.get(it.name.as_str()) {
            let target = target.clone();
            self.record_reference(&target, it.span);
        }
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        if let Some(scope) = self.resolve_namespace(&it.object) {
            if let Some(target) = self
                .table
                .lookup(&scope, it.property.name.as_str())
                .cloned()
            {
                self.record_reference(&target, it.property.span);
            }
        }
        walk::walk_static_member_expression(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        self.scan_catch_tags(it);
        self.strong_depth += 1;
        walk::walk_call_expression(self, it);
        self.strong_depth -= 1;
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        self.strong_depth += 1;
        walk::walk_new_expression(self, it);
        self.strong_depth -= 1;
    }

    fn visit_return_statement(&mut self, it: &ReturnStatement<'a>) {
        self.strong_depth += 1;
        walk::walk_return_statement(self, it);
        self.strong_depth -= 1;
    }

    fn visit_yield_expression(&mut self, it: &YieldExpression<'a>) {
        self.strong_depth += 1;
        walk::walk_yield_expression(self, it);
        self.strong_depth -= 1;
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        self.strong_depth += 1;
        walk::walk_variable_declarator(self, it);
        self.strong_depth -= 1;
    }

    fn visit_ts_type_annotation(&mut self, it: &TSTypeAnnotation<'a>) {
        self.type_depth += 1;
        walk::walk_ts_type_annotation(self, it);
        self.type_depth -= 1;
    }

    fn visit_ts_type(&mut self, it: &TSType<'a>) {
        self.type_depth += 1;
        walk::walk_ts_type(self, it);
        self.type_depth -= 1;
    }
}
