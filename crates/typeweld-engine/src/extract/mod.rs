//! The static extractor: Rust sources to [`Contract`], no compilation.
//!
//! Validation rules are shared with the proc macros through `typeweld-syntax`,
//! so anything the extractor rejects also fails to compile and vice versa.
//! What the macros cannot see — cross-file type resolution — is enforced at
//! compile time by `ApiBound` trait assertions, and reported here as precise
//! diagnostics.
//!
//! Note: extraction parses files with `proc-macro2` span locations, which
//! accumulate in a thread-local source map. Long-lived callers (the language
//! server) should run [`extract`] on short-lived worker threads so that
//! memory is reclaimed between rounds.

mod modules;
mod resolve;
mod router;

use std::collections::{BTreeMap, HashMap, VecDeque};

use typeweld_syntax::{
    analyze_endpoint, apply_rename_rule, default_ts_module, doc_string, reject_generics,
    to_camel_case, ApiAttr, FieldWireShape, ParamKind, ResponseKind, SerdeAttrs, StatusAttr,
};

use crate::diag::Diagnostic;
use crate::ir::{
    Contract, DeclSpans, Endpoint, ErrorDecl, ErrorRef, ErrorVariant, ExternalType, Field,
    HttpMethod, Optionality, PackageMeta, Param, Primitive, RequestBody, RequestShape, Span,
    SuccessBody, SuccessChannel, SuccessTransport, SymbolId, TypeDecl, TypeExpr, TypeRef,
    TypeShape, CONTRACT_SCHEMA_VERSION,
};
use crate::vfs::FileProvider;
use crate::workspace::Workspace;

use modules::{span_of, ModuleMap};
use resolve::{Entity, Resolver};

/// Extraction result: a (possibly partial) contract plus diagnostics.
pub struct Extraction {
    pub contract: Contract,
    pub diagnostics: Vec<Diagnostic>,
}

impl Extraction {
    /// Whether any diagnostic is an error.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == crate::diag::Severity::Error)
    }
}

/// Extracts the contract for `package` (and the workspace crates it reaches).
///
/// # Errors
/// Returns a diagnostic when `package` is not a workspace member; all other
/// problems are reported in [`Extraction::diagnostics`].
pub fn extract(
    workspace: &Workspace,
    package: &str,
    ts_package: &str,
    files: &dyn FileProvider,
) -> Result<Extraction, Diagnostic> {
    if workspace.package(package).is_none() {
        return Err(Diagnostic::error(
            "unknown-package",
            format!("package `{package}` is not a member of this workspace"),
            None,
        ));
    }

    let mut diagnostics = Vec::new();
    let mut modules = ModuleMap::default();
    let reachable = workspace.reachable_packages(package);
    for member in &reachable {
        modules.walk_package(&workspace.root, member, files, &mut diagnostics);
    }
    let crate_idents: Vec<String> = reachable
        .iter()
        .map(|member| member.ident.clone())
        .collect();

    let mut cx = Lowering {
        resolver: Resolver {
            modules: &modules,
            crate_idents,
        },
        diagnostics,
        types: BTreeMap::new(),
        errors: BTreeMap::new(),
        queue: VecDeque::new(),
        lowering_stack: Vec::new(),
    };

    // Every Api/ApiError item is validated (matching the macros, which
    // validate every derive site), but the emitted contract only contains
    // declarations reachable from endpoints.
    cx.lower_all_decls();
    let mut endpoints = cx.lower_endpoints();
    cx.drain_type_queue();

    let routers = router::analyze(&cx.resolver, &mut cx.diagnostics);
    router::apply_mounts(&routers, &mut endpoints, &mut cx.diagnostics);

    endpoints.sort_by(|a, b| (&a.ts_module, &a.ts_name).cmp(&(&b.ts_module, &b.ts_name)));
    let reachable = reachable_ids(&endpoints, &cx.types, &cx.errors);
    let mut types: Vec<TypeDecl> = cx
        .types
        .into_values()
        .filter(|decl| reachable.contains(&decl.id))
        .collect();
    types.sort_by(|a, b| a.ts_name.cmp(&b.ts_name).then_with(|| a.id.cmp(&b.id)));
    let mut errors: Vec<ErrorDecl> = cx
        .errors
        .into_values()
        .filter(|decl| reachable.contains(&decl.id))
        .collect();
    errors.sort_by(|a, b| a.ts_name.cmp(&b.ts_name).then_with(|| a.id.cmp(&b.id)));

    let mut diagnostics = cx.diagnostics;
    check_name_collisions(&endpoints, &types, &errors, &mut diagnostics);

    Ok(Extraction {
        contract: Contract {
            schema_version: CONTRACT_SCHEMA_VERSION,
            package: PackageMeta {
                ts_package: ts_package.to_owned(),
                cargo_package: package.to_owned(),
            },
            endpoints,
            types,
            errors,
        },
        diagnostics,
    })
}

/// What kind of API declaration an item is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApiItemKind {
    Type,
    Error,
}

struct Lowering<'a> {
    resolver: Resolver<'a>,
    diagnostics: Vec<Diagnostic>,
    types: BTreeMap<SymbolId, TypeDecl>,
    errors: BTreeMap<SymbolId, ErrorDecl>,
    /// Named declarations referenced but not yet lowered.
    queue: VecDeque<(Vec<String>, String)>,
    /// Guards against type-alias cycles during lowering.
    lowering_stack: Vec<(Vec<String>, String)>,
}

impl Lowering<'_> {
    fn push_syn_error(&mut self, file: &str, error: &syn::Error) {
        for child in error.clone() {
            let range = child.span().byte_range();
            self.diagnostics.push(Diagnostic::error(
                "invalid-api-item",
                child.to_string(),
                Some(Span {
                    file: file.to_owned(),
                    start: u32::try_from(range.start).unwrap_or(0),
                    end: u32::try_from(range.end).unwrap_or(0),
                }),
            ));
        }
    }

    // ===== Endpoints =====

    fn lower_endpoints(&mut self) -> Vec<Endpoint> {
        let mut endpoints = Vec::new();
        let module_keys: Vec<Vec<String>> = self.resolver.modules.modules.keys().cloned().collect();

        for module_key in module_keys {
            let data = &self.resolver.modules.modules[&module_key];
            let file = data.file.clone();
            let items: Vec<syn::ItemFn> = data
                .items
                .iter()
                .filter_map(|item| match item {
                    syn::Item::Fn(item) if find_attr(&item.attrs, "api").is_some() => {
                        Some(item.clone())
                    }
                    _ => None,
                })
                .collect();

            for item in items {
                if let Some(endpoint) = self.lower_endpoint(&module_key, &file, &item) {
                    endpoints.push(endpoint);
                }
            }
        }

        endpoints
    }

    fn lower_endpoint(
        &mut self,
        module_key: &[String],
        file: &str,
        item: &syn::ItemFn,
    ) -> Option<Endpoint> {
        let attr = find_attr(&item.attrs, "api").expect("filtered above");
        let api_attr = match ApiAttr::from_attribute(attr) {
            Ok(api_attr) => api_attr,
            Err(error) => {
                self.push_syn_error(file, &error);
                return None;
            }
        };
        let sig = match analyze_endpoint(&api_attr, &item.sig) {
            Ok(sig) => sig,
            Err(error) => {
                self.push_syn_error(file, &error);
                return None;
            }
        };

        let mut rust_path = module_key.to_vec();
        let rust_name = item.sig.ident.to_string();
        rust_path.push(rust_name.clone());
        let parts: Vec<&str> = rust_path.iter().map(String::as_str).collect();
        let id = SymbolId::from_parts("endpoint", &parts);

        let mut path_params = Vec::new();
        let mut query_params = Vec::new();
        let mut body = None;
        for param in &sig.params {
            match param.kind {
                ParamKind::Path => {
                    let span = byte_span(file, param.name_span.byte_range());
                    if let Some(ty) = self.lower_scalar(module_key, file, param.inner_ty) {
                        path_params.push(Param {
                            name: param.name.clone(),
                            ty,
                            required: true,
                            span,
                        });
                    }
                }
                ParamKind::Query => {
                    query_params.extend(self.lower_query_struct(module_key, file, param.inner_ty));
                }
                ParamKind::Body => {
                    body = self
                        .lower_type(module_key, file, param.inner_ty)
                        .map(RequestBody::Json);
                }
                ParamKind::BinaryBody => {
                    body = Some(RequestBody::Binary);
                }
            }
        }

        let success = match sig.response {
            ResponseKind::Empty => SuccessChannel {
                transport: SuccessTransport::Unary,
                status: 204,
                body: SuccessBody::Empty,
            },
            ResponseKind::Json(ty) => SuccessChannel {
                transport: SuccessTransport::Unary,
                status: 200,
                body: self
                    .lower_type(module_key, file, ty)
                    .map_or(SuccessBody::Empty, SuccessBody::Json),
            },
            ResponseKind::Created(ty) => SuccessChannel {
                transport: SuccessTransport::Unary,
                status: 201,
                body: self
                    .lower_type(module_key, file, ty)
                    .map_or(SuccessBody::Empty, SuccessBody::Json),
            },
            ResponseKind::Sse(ty) => SuccessChannel {
                transport: SuccessTransport::Sse,
                status: 200,
                body: self
                    .lower_type(module_key, file, ty)
                    .map_or(SuccessBody::Empty, SuccessBody::Stream),
            },
            ResponseKind::Binary => SuccessChannel {
                transport: SuccessTransport::BinaryDownload,
                status: 200,
                body: SuccessBody::Binary,
            },
        };
        let success = if matches!(body, Some(RequestBody::Binary)) {
            SuccessChannel {
                transport: SuccessTransport::BinaryUpload,
                ..success
            }
        } else {
            success
        };

        let errors = sig
            .error
            .and_then(|error_ty| self.lower_error_ref(module_key, file, error_ty))
            .into_iter()
            .collect();

        let method = match api_attr.method.http_method() {
            "GET" => HttpMethod::Get,
            "POST" => HttpMethod::Post,
            "PUT" => HttpMethod::Put,
            "PATCH" => HttpMethod::Patch,
            _ => HttpMethod::Delete,
        };

        Some(Endpoint {
            id,
            rust_name: rust_name.clone(),
            ts_module: default_ts_module(&api_attr.path),
            ts_name: to_camel_case(&rust_name),
            route: api_attr.path.clone(),
            method,
            request: RequestShape {
                path_params,
                query_params,
                body,
            },
            success,
            errors,
            docs: doc_string(&item.attrs),
            spans: DeclSpans {
                name: byte_span(file, item.sig.ident.span().byte_range()),
                full: span_of(file, item),
                attr: Some(span_of(file, attr)),
            },
            router_mounts: Vec::new(),
            allow_unused: api_attr.allow_unused,
            rust_path,
        })
    }

    /// Lowers a `Query<T>` struct: its fields become the query parameters.
    fn lower_query_struct(
        &mut self,
        module_key: &[String],
        file: &str,
        ty: &syn::Type,
    ) -> Vec<Param> {
        let Some((target_module, name)) = self.resolve_named(module_key, ty) else {
            self.unresolved_type(file, ty, "query structs must derive Api in this workspace");
            return Vec::new();
        };
        let Some((item, item_file)) = self.find_item(&target_module, &name) else {
            self.unresolved_type(file, ty, "query structs must derive Api in this workspace");
            return Vec::new();
        };
        let syn::Item::Struct(item) = item else {
            self.diagnostics.push(Diagnostic::error(
                "invalid-query-type",
                "Query<T> requires a named-field struct deriving Api",
                Some(span_of(file, ty)),
            ));
            return Vec::new();
        };
        let syn::Fields::Named(fields) = &item.fields else {
            self.diagnostics.push(Diagnostic::error(
                "invalid-query-type",
                "Query<T> requires a named-field struct deriving Api",
                Some(span_of(&item_file, &item.ident)),
            ));
            return Vec::new();
        };

        let container_serde = match SerdeAttrs::from_attrs(&item.attrs) {
            Ok(serde) => serde,
            Err(error) => {
                self.push_syn_error(&item_file, &error);
                return Vec::new();
            }
        };

        let mut params = Vec::new();
        for field in &fields.named {
            let Some(ident) = &field.ident else { continue };
            let serde = match SerdeAttrs::from_attrs(&field.attrs) {
                Ok(serde) => serde,
                Err(error) => {
                    self.push_syn_error(&item_file, &error);
                    continue;
                }
            };
            let shape = match FieldWireShape::from_type(&field.ty, &serde) {
                Ok(shape) => shape,
                Err(error) => {
                    self.push_syn_error(&item_file, &error);
                    continue;
                }
            };
            let wire_name = serde.rename.clone().unwrap_or_else(|| {
                apply_rename_rule(&ident.to_string(), container_serde.rename_all)
            });
            let Some(ty) = self.lower_scalar(&target_module, &item_file, shape.inner_ty) else {
                continue;
            };
            let required = matches!(
                shape.optionality,
                typeweld_syntax::WireOptionality::Required
            );
            params.push(Param {
                name: wire_name,
                ty,
                required,
                span: byte_span(&item_file, ident.span().byte_range()),
            });
        }
        params
    }

    /// Lowers a type that must be a string-encodable scalar (path and query
    /// parameter position).
    fn lower_scalar(
        &mut self,
        module_key: &[String],
        file: &str,
        ty: &syn::Type,
    ) -> Option<TypeExpr> {
        let lowered = self.lower_type(module_key, file, ty)?;
        match &lowered {
            TypeExpr::Primitive(_)
            | TypeExpr::External(
                ExternalType::Uuid | ExternalType::DateTimeUtc | ExternalType::Decimal,
            ) => Some(lowered),
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    "invalid-param-type",
                    "path and query parameters must be primitives, Uuid, DateTime<Utc>, or \
                     Decimal",
                    Some(span_of(file, ty)),
                ));
                None
            }
        }
    }

    fn lower_error_ref(
        &mut self,
        module_key: &[String],
        file: &str,
        ty: &syn::Type,
    ) -> Option<ErrorRef> {
        let Some((target_module, name)) = self.resolve_named(module_key, ty) else {
            self.unresolved_type(file, ty, "endpoint errors must derive ApiError");
            return None;
        };
        let Some((item, _)) = self.find_item(&target_module, &name) else {
            self.unresolved_type(file, ty, "endpoint errors must derive ApiError");
            return None;
        };
        if api_item_kind(&item) != Some(ApiItemKind::Error) {
            self.diagnostics.push(Diagnostic::error(
                "invalid-error-type",
                format!("`{name}` is not an ApiError enum; derive ApiError on it"),
                Some(span_of(file, ty)),
            ));
            return None;
        }

        let mut rust_path = target_module.clone();
        rust_path.push(name.clone());
        let parts: Vec<&str> = rust_path.iter().map(String::as_str).collect();
        self.enqueue(target_module, name.clone());
        Some(ErrorRef {
            id: SymbolId::from_parts("error", &parts),
            name,
        })
    }

    // ===== Types =====

    fn enqueue(&mut self, module: Vec<String>, name: String) {
        self.queue.push_back((module, name));
    }

    fn drain_type_queue(&mut self) {
        while let Some((module, name)) = self.queue.pop_front() {
            self.lower_named_decl(&module, &name);
        }
    }

    /// Lowers (and thereby validates) every Api/ApiError item in the walked
    /// modules, whether or not an endpoint references it.
    fn lower_all_decls(&mut self) {
        let mut decls: Vec<(Vec<String>, String)> = Vec::new();
        for (module_key, data) in &self.resolver.modules.modules {
            for item in &data.items {
                if api_item_kind(item).is_some() {
                    let name = match item {
                        syn::Item::Struct(item) => item.ident.to_string(),
                        syn::Item::Enum(item) => item.ident.to_string(),
                        _ => continue,
                    };
                    decls.push((module_key.clone(), name));
                }
            }
        }
        decls.sort();
        for (module, name) in decls {
            self.lower_named_decl(&module, &name);
        }
        self.drain_type_queue();
    }

    fn lower_named_decl(&mut self, module: &[String], name: &str) {
        let Some((item, file)) = self.find_item(module, name) else {
            return;
        };
        match api_item_kind(&item) {
            Some(ApiItemKind::Type) => self.lower_type_decl(module, &file, &item),
            Some(ApiItemKind::Error) => {
                let syn::Item::Enum(item) = &item else { return };
                self.lower_error_decl(module, &file, item);
            }
            None => {}
        }
    }

    fn lower_type_decl(&mut self, module: &[String], file: &str, item: &syn::Item) {
        let (ident, attrs, generics) = match item {
            syn::Item::Struct(item) => (&item.ident, &item.attrs, &item.generics),
            syn::Item::Enum(item) => (&item.ident, &item.attrs, &item.generics),
            _ => return,
        };
        let rust_name = ident.to_string();
        let mut rust_path = module.to_vec();
        rust_path.push(rust_name.clone());
        let parts: Vec<&str> = rust_path.iter().map(String::as_str).collect();
        let id = SymbolId::from_parts("type", &parts);
        if self.types.contains_key(&id) {
            return;
        }
        // Reserve the slot to break reference cycles.
        let placeholder = TypeDecl {
            id: id.clone(),
            rust_path: rust_path.clone(),
            rust_name: rust_name.clone(),
            ts_name: rust_name.clone(),
            shape: TypeShape::Struct { fields: Vec::new() },
            docs: doc_string(attrs),
            spans: DeclSpans {
                name: byte_span(file, ident.span().byte_range()),
                full: span_of(file, item),
                attr: None,
            },
        };
        self.types.insert(id.clone(), placeholder);

        if let Err(error) = reject_generics(generics, "Api types") {
            self.push_syn_error(file, &error);
            return;
        }
        let container_serde = match SerdeAttrs::from_attrs(attrs) {
            Ok(serde) => serde,
            Err(error) => {
                self.push_syn_error(file, &error);
                return;
            }
        };

        let owner = rust_path.join("::");
        let shape = match item {
            syn::Item::Struct(item) => {
                self.lower_struct_shape(module, file, item, &container_serde, &owner)
            }
            syn::Item::Enum(item) => {
                self.lower_enum_shape(module, file, item, &container_serde, &owner)
            }
            _ => None,
        };
        if let Some(shape) = shape {
            if let Some(decl) = self.types.get_mut(&id) {
                decl.shape = shape;
            }
        }
    }

    fn lower_struct_shape(
        &mut self,
        module: &[String],
        file: &str,
        item: &syn::ItemStruct,
        container_serde: &SerdeAttrs,
        owner: &str,
    ) -> Option<TypeShape> {
        match &item.fields {
            syn::Fields::Named(fields) => {
                if let Err(error) = container_serde.validate_struct_container("Api", &item.ident) {
                    self.push_syn_error(file, &error);
                    return None;
                }
                if container_serde.transparent {
                    if fields.named.len() != 1 {
                        self.diagnostics.push(Diagnostic::error(
                            "invalid-api-item",
                            "serde transparent Api structs must contain exactly one field",
                            Some(byte_span(file, item.ident.span().byte_range())),
                        ));
                        return None;
                    }
                    let field = fields.named.first().expect("length checked");
                    let field_serde = match SerdeAttrs::from_attrs(&field.attrs) {
                        Ok(serde) => serde,
                        Err(error) => {
                            self.push_syn_error(file, &error);
                            return None;
                        }
                    };
                    if let Err(error) = field_serde.validate_transparent_field("Api", field) {
                        self.push_syn_error(file, &error);
                        return None;
                    }
                    let inner = self.lower_type(module, file, &field.ty)?;
                    return Some(TypeShape::Newtype(inner));
                }

                let lowered = self.lower_fields(
                    module,
                    file,
                    fields.named.iter(),
                    container_serde.rename_all,
                    owner,
                    &item.ident.to_string(),
                );
                Some(TypeShape::Struct { fields: lowered })
            }
            syn::Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                if let Err(error) = container_serde.validate_newtype_container("Api", &item.ident) {
                    self.push_syn_error(file, &error);
                    return None;
                }
                let field = fields.unnamed.first().expect("length checked");
                let inner = self.lower_type(module, file, &field.ty)?;
                Some(TypeShape::Newtype(inner))
            }
            syn::Fields::Unnamed(_) | syn::Fields::Unit => {
                self.diagnostics.push(Diagnostic::error(
                    "invalid-api-item",
                    "Api tuple structs must contain exactly one field",
                    Some(byte_span(file, item.ident.span().byte_range())),
                ));
                None
            }
        }
    }

    fn lower_enum_shape(
        &mut self,
        module: &[String],
        file: &str,
        item: &syn::ItemEnum,
        container_serde: &SerdeAttrs,
        owner: &str,
    ) -> Option<TypeShape> {
        if let Err(error) = container_serde.validate_enum_container("Api", &item.ident) {
            self.push_syn_error(file, &error);
            return None;
        }

        let mut variants = Vec::new();
        for variant in &item.variants {
            if let Some(lowered) =
                self.lower_enum_variant(module, file, variant, container_serde, owner, "Api")
            {
                variants.push(lowered);
            }
        }
        Some(TypeShape::Enum { variants })
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_enum_variant(
        &mut self,
        module: &[String],
        file: &str,
        variant: &syn::Variant,
        container_serde: &SerdeAttrs,
        owner: &str,
        derive: &str,
    ) -> Option<crate::ir::EnumVariant> {
        let variant_name = variant.ident.to_string();
        let variant_serde = match SerdeAttrs::from_attrs(&variant.attrs) {
            Ok(serde) => serde,
            Err(error) => {
                self.push_syn_error(file, &error);
                return None;
            }
        };
        if let Err(error) = variant_serde.validate_enum_variant(derive, variant) {
            self.push_syn_error(file, &error);
            return None;
        }
        let field_rename_all = variant_serde
            .rename_all
            .or(container_serde.rename_all_fields);
        let wire_name = variant_serde
            .rename
            .clone()
            .unwrap_or_else(|| apply_rename_rule(&variant_name, container_serde.rename_all));

        let fields = match &variant.fields {
            syn::Fields::Unit => Vec::new(),
            syn::Fields::Named(fields) => self.lower_fields(
                module,
                file,
                fields.named.iter(),
                field_rename_all,
                owner,
                &variant_name,
            ),
            syn::Fields::Unnamed(_) => {
                self.diagnostics.push(Diagnostic::error(
                    "invalid-api-item",
                    format!("{derive} enum variants must be unit or named-field variants"),
                    Some(span_of(file, variant)),
                ));
                return None;
            }
        };

        Some(crate::ir::EnumVariant {
            id: SymbolId::from_parts("enum_variant", &[owner, &variant_name]),
            rust_name: variant_name,
            wire_name,
            fields,
            docs: doc_string(&variant.attrs),
            spans: DeclSpans {
                name: byte_span(file, variant.ident.span().byte_range()),
                full: span_of(file, variant),
                attr: None,
            },
        })
    }

    fn lower_fields<'f>(
        &mut self,
        module: &[String],
        file: &str,
        fields: impl Iterator<Item = &'f syn::Field>,
        rename_all: Option<typeweld_syntax::RenameRule>,
        owner: &str,
        container: &str,
    ) -> Vec<Field> {
        let mut lowered = Vec::new();
        for field in fields {
            let Some(ident) = &field.ident else { continue };
            let field_name = ident.to_string();
            let serde = match SerdeAttrs::from_attrs(&field.attrs) {
                Ok(serde) => serde,
                Err(error) => {
                    self.push_syn_error(file, &error);
                    continue;
                }
            };
            if let Err(error) = serde.validate_field("Api", field) {
                self.push_syn_error(file, &error);
                continue;
            }
            let shape = match FieldWireShape::from_type(&field.ty, &serde) {
                Ok(shape) => shape,
                Err(error) => {
                    self.push_syn_error(file, &error);
                    continue;
                }
            };
            let Some(ty) = self.lower_type(module, file, shape.inner_ty) else {
                continue;
            };
            let wire_name = serde
                .rename
                .clone()
                .unwrap_or_else(|| apply_rename_rule(&field_name, rename_all));
            let optionality = match shape.optionality {
                typeweld_syntax::WireOptionality::Required => Optionality::Required,
                typeweld_syntax::WireOptionality::Optional => Optionality::Optional,
                typeweld_syntax::WireOptionality::Nullable => Optionality::Nullable,
                typeweld_syntax::WireOptionality::OptionalNullable => Optionality::OptionalNullable,
            };

            lowered.push(Field {
                id: SymbolId::from_parts("field", &[owner, container, &field_name]),
                rust_name: field_name,
                wire_name,
                ty,
                optionality,
                docs: doc_string(&field.attrs),
                spans: DeclSpans {
                    name: byte_span(file, ident.span().byte_range()),
                    full: span_of(file, field),
                    attr: None,
                },
            });
        }
        lowered
    }

    fn lower_error_decl(&mut self, module: &[String], file: &str, item: &syn::ItemEnum) {
        let rust_name = item.ident.to_string();
        let mut rust_path = module.to_vec();
        rust_path.push(rust_name.clone());
        let parts: Vec<&str> = rust_path.iter().map(String::as_str).collect();
        let id = SymbolId::from_parts("error", &parts);
        if self.errors.contains_key(&id) {
            return;
        }
        let placeholder = ErrorDecl {
            id: id.clone(),
            rust_path: rust_path.clone(),
            rust_name: rust_name.clone(),
            ts_name: rust_name.clone(),
            variants: Vec::new(),
            docs: doc_string(&item.attrs),
            spans: DeclSpans {
                name: byte_span(file, item.ident.span().byte_range()),
                full: span_of(file, item),
                attr: None,
            },
        };
        self.errors.insert(id.clone(), placeholder);

        if let Err(error) = reject_generics(&item.generics, "ApiError enums") {
            self.push_syn_error(file, &error);
            return;
        }
        let container_serde = match SerdeAttrs::from_attrs(&item.attrs) {
            Ok(serde) => serde,
            Err(error) => {
                self.push_syn_error(file, &error);
                return;
            }
        };
        if let Err(error) = container_serde.validate_enum_container("ApiError", &item.ident) {
            self.push_syn_error(file, &error);
            return;
        }

        let owner = rust_path.join("::");
        let mut variants = Vec::new();
        for variant in &item.variants {
            let status = match StatusAttr::from_variant(variant) {
                Ok(status) => status.status,
                Err(error) => {
                    self.push_syn_error(file, &error);
                    continue;
                }
            };
            let Some(lowered) = self.lower_enum_variant(
                module,
                file,
                variant,
                &container_serde,
                &owner,
                "ApiError",
            ) else {
                continue;
            };
            variants.push(ErrorVariant {
                id: SymbolId::from_parts("error_variant", &[&owner, &lowered.rust_name]),
                rust_name: lowered.rust_name,
                tag: lowered.wire_name,
                status,
                fields: lowered.fields,
                docs: lowered.docs,
                spans: lowered.spans,
            });
        }
        if let Some(decl) = self.errors.get_mut(&id) {
            decl.variants = variants;
        }
    }

    // ===== Type expression lowering =====

    fn lower_type(&mut self, module: &[String], file: &str, ty: &syn::Type) -> Option<TypeExpr> {
        match ty {
            syn::Type::Paren(inner) => self.lower_type(module, file, &inner.elem),
            syn::Type::Group(inner) => self.lower_type(module, file, &inner.elem),
            syn::Type::Path(path) if path.qself.is_none() => {
                self.lower_type_path(module, file, ty, &path.path)
            }
            _ => {
                self.unresolved_type(file, ty, "use a supported API type");
                None
            }
        }
    }

    fn lower_type_path(
        &mut self,
        module: &[String],
        file: &str,
        ty: &syn::Type,
        path: &syn::Path,
    ) -> Option<TypeExpr> {
        let last = path.segments.last()?;
        let last_ident = last.ident.to_string();

        // Builtin wrappers, matched structurally on the last segment.
        if let syn::PathArguments::AngleBracketed(args) = &last.arguments {
            let type_args: Vec<&syn::Type> = args
                .args
                .iter()
                .filter_map(|arg| match arg {
                    syn::GenericArgument::Type(inner) => Some(inner),
                    _ => None,
                })
                .collect();
            match last_ident.as_str() {
                "Vec" if type_args.len() == 1 => {
                    let inner = self.lower_type(module, file, type_args[0])?;
                    return Some(TypeExpr::List(Box::new(inner)));
                }
                "Option" if type_args.len() == 1 => {
                    let inner = self.lower_type(module, file, type_args[0])?;
                    return Some(TypeExpr::Option(Box::new(inner)));
                }
                "Box" if type_args.len() == 1 => {
                    return self.lower_type(module, file, type_args[0]);
                }
                "HashMap" | "BTreeMap" if type_args.len() == 2 => {
                    let key = self.lower_type(module, file, type_args[0])?;
                    if !matches!(key, TypeExpr::Primitive(Primitive::String)) {
                        self.diagnostics.push(Diagnostic::error(
                            "invalid-map-key",
                            "API map keys must be String",
                            Some(span_of(file, type_args[0])),
                        ));
                        return None;
                    }
                    let value = self.lower_type(module, file, type_args[1])?;
                    return Some(TypeExpr::Map {
                        key: Box::new(key),
                        value: Box::new(value),
                    });
                }
                "DateTime" if type_args.len() == 1 => {
                    let resolved = self.resolve_stripped(module, path);
                    if resolved != Entity::External(ExternalType::DateTimeUtc) {
                        self.unresolved_type(file, ty, "use chrono::DateTime<Utc>");
                        return None;
                    }
                    let arg_ok = matches!(
                        type_args[0],
                        syn::Type::Path(arg) if arg
                            .path
                            .segments
                            .last()
                            .is_some_and(|segment| segment.ident == "Utc")
                    );
                    if !arg_ok {
                        self.diagnostics.push(Diagnostic::error(
                            "invalid-datetime",
                            "only chrono::DateTime<Utc> is supported",
                            Some(span_of(file, ty)),
                        ));
                        return None;
                    }
                    return Some(TypeExpr::External(ExternalType::DateTimeUtc));
                }
                _ => {
                    self.unresolved_type(file, ty, "generic API types are not supported");
                    return None;
                }
            }
        }

        // Primitives win over any same-named local item.
        if path.segments.len() == 1 {
            if let Some(primitive) = Primitive::from_rust_name(&last_ident) {
                if primitive.is_big_int() {
                    self.diagnostics.push(Diagnostic::warning(
                        "imprecise-int",
                        format!(
                            "`{last_ident}` maps to TypeScript `number`, which loses precision \
                             beyond 2^53; consider i32/u32 or a string-typed newtype"
                        ),
                        Some(span_of(file, ty)),
                    ));
                }
                return Some(TypeExpr::Primitive(primitive));
            }
        }

        let segments: Vec<String> = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        match self.resolver.resolve_path(module, &segments) {
            Entity::External(external) => Some(TypeExpr::External(external)),
            Entity::Item(target_module, name) => {
                self.lower_named_reference(module, file, ty, &target_module, &name)
            }
            _ => {
                // `std::string::String` and friends.
                if let Some(primitive) = fully_qualified_primitive(&segments) {
                    return Some(TypeExpr::Primitive(primitive));
                }
                self.unresolved_type(
                    file,
                    ty,
                    "derive Api on it (in this workspace) or use a supported type",
                );
                None
            }
        }
    }

    fn lower_named_reference(
        &mut self,
        _module: &[String],
        file: &str,
        ty: &syn::Type,
        target_module: &[String],
        name: &str,
    ) -> Option<TypeExpr> {
        let Some((item, item_file)) = self.find_item(target_module, name) else {
            self.unresolved_type(file, ty, "item not found");
            return None;
        };

        match &item {
            syn::Item::Type(alias) => {
                let key = (target_module.to_vec(), name.to_owned());
                if self.lowering_stack.contains(&key) {
                    self.diagnostics.push(Diagnostic::error(
                        "alias-cycle",
                        format!("type alias cycle while resolving `{name}`"),
                        Some(span_of(file, ty)),
                    ));
                    return None;
                }
                self.lowering_stack.push(key);
                let lowered = self.lower_type(target_module, &item_file, &alias.ty);
                self.lowering_stack.pop();
                lowered
            }
            _ => {
                if let Some(kind) = api_item_kind(&item) {
                    let mut rust_path = target_module.to_vec();
                    rust_path.push(name.to_owned());
                    let parts: Vec<&str> = rust_path.iter().map(String::as_str).collect();
                    let namespace = match kind {
                        ApiItemKind::Type => "type",
                        ApiItemKind::Error => "error",
                    };
                    self.enqueue(target_module.to_vec(), name.to_owned());
                    Some(TypeExpr::Named(TypeRef {
                        id: SymbolId::from_parts(namespace, &parts),
                        name: name.to_owned(),
                    }))
                } else {
                    self.diagnostics.push(Diagnostic::error(
                        "missing-api-derive",
                        format!(
                            "`{name}` is used in an API position but does not derive Api or \
                         ApiError"
                        ),
                        Some(span_of(file, ty)),
                    ));
                    None
                }
            }
        }
    }

    /// Resolves a path with the generic arguments stripped.
    fn resolve_stripped(&self, module: &[String], path: &syn::Path) -> Entity {
        let segments: Vec<String> = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        self.resolver.resolve_path(module, &segments)
    }

    fn resolve_named(
        &mut self,
        module: &[String],
        ty: &syn::Type,
    ) -> Option<(Vec<String>, String)> {
        let syn::Type::Path(path) = ty else {
            return None;
        };
        if path.qself.is_some() {
            return None;
        }
        let segments: Vec<String> = path
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        match self.resolver.resolve_path(module, &segments) {
            Entity::Item(module, name) => Some((module, name)),
            _ => None,
        }
    }

    fn find_item(&self, module: &[String], name: &str) -> Option<(syn::Item, String)> {
        let data = self.resolver.modules.modules.get(module)?;
        let item = data.items.iter().find(|item| match item {
            syn::Item::Struct(item) => item.ident == name,
            syn::Item::Enum(item) => item.ident == name,
            syn::Item::Type(item) => item.ident == name,
            syn::Item::Fn(item) => item.sig.ident == name,
            _ => false,
        })?;
        Some((item.clone(), data.file.clone()))
    }

    fn unresolved_type(&mut self, file: &str, ty: &syn::Type, hint: &str) {
        self.diagnostics.push(Diagnostic::error(
            "unresolved-type",
            format!(
                "cannot resolve `{}` in an API position; {hint}",
                type_to_string(ty)
            ),
            Some(span_of(file, ty)),
        ));
    }
}

/// Returns the `#[api]`-style attribute matching `name` by last path segment.
fn find_attr<'a>(attrs: &'a [syn::Attribute], name: &str) -> Option<&'a syn::Attribute> {
    attrs.iter().find(|attr| {
        attr.path()
            .segments
            .last()
            .is_some_and(|segment| segment.ident == name)
    })
}

/// Classifies an item by its typeweld derives.
fn api_item_kind(item: &syn::Item) -> Option<ApiItemKind> {
    let attrs = match item {
        syn::Item::Struct(item) => &item.attrs,
        syn::Item::Enum(item) => &item.attrs,
        _ => return None,
    };
    let mut kind = None;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("derive")) {
        let _ = attr.parse_nested_meta(|meta| {
            let Some(last) = meta.path.segments.last() else {
                return Ok(());
            };
            if last.ident == "Api" {
                kind = Some(ApiItemKind::Type);
            } else if last.ident == "ApiError" {
                kind = Some(ApiItemKind::Error);
            }
            Ok(())
        });
    }
    kind
}

fn fully_qualified_primitive(segments: &[String]) -> Option<Primitive> {
    let parts: Vec<&str> = segments.iter().map(String::as_str).collect();
    match parts.as_slice() {
        ["std" | "alloc", "string", "String"] => Some(Primitive::String),
        _ => None,
    }
}

fn byte_span(file: &str, range: std::ops::Range<usize>) -> Span {
    Span {
        file: file.to_owned(),
        start: u32::try_from(range.start).unwrap_or(0),
        end: u32::try_from(range.end).unwrap_or(0),
    }
}

fn type_to_string(ty: &syn::Type) -> String {
    use quote::ToTokens as _;
    ty.to_token_stream().to_string().replace(' ', "")
}

/// Collects the ids of all declarations reachable from the endpoints.
fn reachable_ids(
    endpoints: &[Endpoint],
    types: &BTreeMap<SymbolId, TypeDecl>,
    errors: &BTreeMap<SymbolId, ErrorDecl>,
) -> std::collections::HashSet<SymbolId> {
    fn collect_expr(expr: &TypeExpr, queue: &mut Vec<SymbolId>) {
        match expr {
            TypeExpr::Named(type_ref) => queue.push(type_ref.id.clone()),
            TypeExpr::Option(inner) | TypeExpr::List(inner) => collect_expr(inner, queue),
            TypeExpr::Map { key, value } => {
                collect_expr(key, queue);
                collect_expr(value, queue);
            }
            TypeExpr::Primitive(_) | TypeExpr::External(_) => {}
        }
    }

    let mut queue = Vec::new();
    for endpoint in endpoints {
        for param in endpoint
            .request
            .path_params
            .iter()
            .chain(&endpoint.request.query_params)
        {
            collect_expr(&param.ty, &mut queue);
        }
        if let Some(RequestBody::Json(expr)) = &endpoint.request.body {
            collect_expr(expr, &mut queue);
        }
        match &endpoint.success.body {
            SuccessBody::Json(expr) | SuccessBody::Stream(expr) => collect_expr(expr, &mut queue),
            SuccessBody::Empty | SuccessBody::Binary => {}
        }
        for error in &endpoint.errors {
            queue.push(error.id.clone());
        }
    }

    let mut seen = std::collections::HashSet::new();
    while let Some(id) = queue.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(decl) = types.get(&id) {
            match &decl.shape {
                TypeShape::Struct { fields } => {
                    for field in fields {
                        collect_expr(&field.ty, &mut queue);
                    }
                }
                TypeShape::Enum { variants } => {
                    for variant in variants {
                        for field in &variant.fields {
                            collect_expr(&field.ty, &mut queue);
                        }
                    }
                }
                TypeShape::Newtype(inner) => collect_expr(inner, &mut queue),
            }
        }
        if let Some(decl) = errors.get(&id) {
            for variant in &decl.variants {
                for field in &variant.fields {
                    collect_expr(&field.ty, &mut queue);
                }
            }
        }
    }
    seen
}

fn check_name_collisions(
    endpoints: &[Endpoint],
    types: &[TypeDecl],
    errors: &[ErrorDecl],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut type_names: HashMap<&str, &SymbolId> = HashMap::new();
    for decl in types {
        if let Some(_existing) = type_names.insert(decl.ts_name.as_str(), &decl.id) {
            diagnostics.push(Diagnostic::error(
                "name-collision",
                format!(
                    "two API types named `{}` exist in different modules; generated TypeScript \
                     names must be unique — rename one of them",
                    decl.ts_name
                ),
                Some(decl.spans.name.clone()),
            ));
        }
    }
    for decl in errors {
        if type_names.contains_key(decl.ts_name.as_str()) {
            diagnostics.push(Diagnostic::error(
                "name-collision",
                format!(
                    "API error `{}` collides with an API type of the same name",
                    decl.ts_name
                ),
                Some(decl.spans.name.clone()),
            ));
        }
    }
    let mut error_names: HashMap<&str, &SymbolId> = HashMap::new();
    for decl in errors {
        if error_names
            .insert(decl.ts_name.as_str(), &decl.id)
            .is_some()
        {
            diagnostics.push(Diagnostic::error(
                "name-collision",
                format!(
                    "two API errors named `{}` exist in different modules; rename one of them",
                    decl.ts_name
                ),
                Some(decl.spans.name.clone()),
            ));
        }
    }

    let mut endpoint_names: HashMap<(&str, &str), &SymbolId> = HashMap::new();
    for endpoint in endpoints {
        if endpoint_names
            .insert(
                (endpoint.ts_module.as_str(), endpoint.ts_name.as_str()),
                &endpoint.id,
            )
            .is_some()
        {
            diagnostics.push(Diagnostic::error(
                "name-collision",
                format!(
                    "two endpoints generate `{}.{}`; rename one of the handler functions",
                    endpoint.ts_module, endpoint.ts_name
                ),
                Some(endpoint.spans.name.clone()),
            ));
        }
    }
}
