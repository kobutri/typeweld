//! Contract hovers for both sides of the language boundary.

use std::fmt::Write as _;

use lsp_types::{Hover, HoverContents, HoverParams, MarkupContent, MarkupKind};
use typeweld_engine::ir::{Endpoint, ErrorDecl, ErrorVariant, Field, Param, TypeDecl};

use crate::convert;
use crate::features::resolve;
use crate::state::{FieldOwner, PackageSnapshot, State, SymbolInfo};

pub fn handle(state: &State, params: &HoverParams) -> Option<Hover> {
    let position = &params.text_document_position_params;
    let resolved = resolve(state, &position.text_document.uri, position.position)?;
    let snapshot = state.snapshot()?;
    let package = &snapshot.packages[resolved.package];
    let symbol = package.symbol_by_id(&resolved.id)?;

    let value = match symbol.info {
        SymbolInfo::Endpoint(index) => render_endpoint(package, &package.contract.endpoints[index]),
        SymbolInfo::Type(index) => render_type(package, &package.contract.types[index]),
        SymbolInfo::Error(index) => render_error(package, &package.contract.errors[index]),
        SymbolInfo::Field { owner, field } => render_field(field_decl(package, owner, field)?),
        SymbolInfo::Variant { ty, variant } => {
            let typeweld_engine::ir::TypeShape::Enum { variants } =
                &package.contract.types[ty].shape
            else {
                return None;
            };
            render_variant(variants.get(variant)?)
        }
        SymbolInfo::ErrorVariant { error, variant } => {
            render_error_variant(package.contract.errors[error].variants.get(variant)?)
        }
        SymbolInfo::Param {
            endpoint,
            query,
            param,
        } => {
            let declaration = &package.contract.endpoints[endpoint];
            let params = if query {
                &declaration.request.query_params
            } else {
                &declaration.request.path_params
            };
            render_param(declaration, params.get(param)?, query)
        }
    };

    let text = state.read_text(&resolved.path)?;
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(convert::range(
            &text,
            resolved.start,
            resolved.end,
            state.encoding(),
        )),
    })
}

fn field_decl(package: &PackageSnapshot, owner: FieldOwner, field: usize) -> Option<&Field> {
    match owner {
        FieldOwner::Struct(ty) => {
            let typeweld_engine::ir::TypeShape::Struct { fields } =
                &package.contract.types[ty].shape
            else {
                return None;
            };
            fields.get(field)
        }
        FieldOwner::Enum { ty, variant } => {
            let typeweld_engine::ir::TypeShape::Enum { variants } =
                &package.contract.types[ty].shape
            else {
                return None;
            };
            variants.get(variant)?.fields.get(field)
        }
        FieldOwner::Error { error, variant } => package.contract.errors[error]
            .variants
            .get(variant)?
            .fields
            .get(field),
    }
}

fn push_docs(out: &mut String, docs: Option<&String>) {
    if let Some(docs) = docs {
        out.push_str("\n\n");
        out.push_str(docs);
    }
}

fn render_endpoint(package: &PackageSnapshot, endpoint: &Endpoint) -> String {
    let mut out = format!(
        "**{} {}**\n\n`{}`",
        endpoint.method.as_str(),
        endpoint.route,
        endpoint.rust_path.join("::")
    );
    push_docs(&mut out, endpoint.docs.as_ref());

    let mut tags = Vec::new();
    for reference in &endpoint.errors {
        let Some(decl) = package
            .contract
            .errors
            .iter()
            .find(|decl| decl.id == reference.id)
        else {
            continue;
        };
        for variant in &decl.variants {
            tags.push(format!("{} ({})", variant.tag, variant.status));
        }
    }
    if !tags.is_empty() {
        let _ = write!(out, "\n\nErrors: {}", tags.join(", "));
    }

    let usages = package.usage.refs_for(&endpoint.id).len();
    let _ = write!(
        out,
        "\n\nTypeScript: `{}.{}` — {usages} usage(s)",
        endpoint.ts_module, endpoint.ts_name
    );
    out
}

fn render_type(package: &PackageSnapshot, decl: &TypeDecl) -> String {
    let mut out = format!("**{}**\n\n`{}`", decl.rust_name, decl.rust_path.join("::"));
    push_docs(&mut out, decl.docs.as_ref());
    let usages = package.usage.refs_for(&decl.id).len();
    let _ = write!(
        out,
        "\n\nTypeScript: `{}` — {usages} usage(s)",
        decl.ts_name
    );
    out
}

fn render_error(package: &PackageSnapshot, decl: &ErrorDecl) -> String {
    let mut out = format!("**{}**\n\n`{}`", decl.rust_name, decl.rust_path.join("::"));
    push_docs(&mut out, decl.docs.as_ref());
    let variants: Vec<String> = decl
        .variants
        .iter()
        .map(|variant| format!("{} ({})", variant.tag, variant.status))
        .collect();
    if !variants.is_empty() {
        let _ = write!(out, "\n\nVariants: {}", variants.join(", "));
    }
    let usages = package.usage.refs_for(&decl.id).len();
    let _ = write!(
        out,
        "\n\nTypeScript: `{}` — {usages} usage(s)",
        decl.ts_name
    );
    out
}

fn render_field(field: &Field) -> String {
    let mut out = format!(
        "**{}**\n\nWire name: `{}`",
        field.rust_name, field.wire_name
    );
    push_docs(&mut out, field.docs.as_ref());
    out
}

fn render_variant(variant: &typeweld_engine::ir::EnumVariant) -> String {
    let mut out = format!("**{}**\n\nTag: `{}`", variant.rust_name, variant.wire_name);
    push_docs(&mut out, variant.docs.as_ref());
    out
}

fn render_error_variant(variant: &ErrorVariant) -> String {
    let mut out = format!(
        "**{}**\n\nTag: `{}` — HTTP {}",
        variant.rust_name, variant.tag, variant.status
    );
    push_docs(&mut out, variant.docs.as_ref());
    out
}

fn render_param(endpoint: &Endpoint, param: &Param, query: bool) -> String {
    format!(
        "**{}**\n\n{} parameter of `{}` ({} {})",
        param.name,
        if query { "Query" } else { "Path" },
        endpoint.rust_name,
        endpoint.method.as_str(),
        endpoint.route
    )
}
