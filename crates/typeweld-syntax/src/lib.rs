//! Shared syn-based parsing and validation for the typeweld macros and the
//! static extractor.
//!
//! Every rule that decides whether an API item is accepted, and how its wire
//! shape is derived, lives in this crate exactly once. The proc macros report
//! violations as compile errors; the extractor reports the same violations as
//! extraction diagnostics. Keeping a single implementation makes drift between
//! "what compiles" and "what extracts" structurally impossible for local rules.

mod api_attr;
mod endpoint;
mod rename;
mod serde_attrs;

pub use api_attr::{ApiAttr, ApiMethod, StatusAttr};
pub use endpoint::{
    analyze_endpoint, reject_generics, validate_endpoint_error_type, EndpointParam, EndpointSig,
    ParamKind, ResponseKind,
};
pub use rename::{apply_rename_rule, split_words, to_camel_case, to_snake_case, RenameRule};
pub use serde_attrs::{FieldWireShape, SerdeAttrs, WireOptionality};

/// Extracts `///` doc comments from a list of attributes, joined by newlines.
#[must_use]
pub fn doc_string(attrs: &[syn::Attribute]) -> Option<String> {
    let lines: Vec<String> = attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| {
            let syn::Meta::NameValue(meta) = &attr.meta else {
                return None;
            };
            let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(value),
                ..
            }) = &meta.value
            else {
                return None;
            };
            Some(value.value().trim().to_owned())
        })
        .collect();

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Extracts the route parameter names from a path pattern.
///
/// Supports both `/users/{id}` and legacy `/users/:id` segment styles.
#[must_use]
pub fn route_params(path: &str) -> Vec<String> {
    let mut params = Vec::new();
    let mut chars = path.char_indices().peekable();

    while let Some((index, character)) = chars.next() {
        if character == '{' {
            let start = index + 1;
            if let Some((end, _)) = chars.by_ref().find(|(_, ch)| *ch == '}') {
                params.push(path[start..end].to_owned());
            }
        } else if character == ':' {
            let start = index + 1;
            let end = chars
                .clone()
                .find(|(_, ch)| *ch == '/' || *ch == '{' || *ch == '}')
                .map_or(path.len(), |(end, _)| end);
            params.push(path[start..end].to_owned());
        }
    }

    params
}

/// Derives the default TypeScript module for an endpoint from its route.
///
/// The first static path segment becomes the module name; endpoints whose
/// routes start with a parameter fall back to `api`.
#[must_use]
pub fn default_ts_module(path: &str) -> String {
    path.split('/')
        .find_map(|segment| {
            let segment = segment.trim();
            (!segment.is_empty()
                && !segment.starts_with('{')
                && !segment.starts_with(':')
                && !segment.contains('}'))
            .then(|| to_camel_case(segment))
        })
        .filter(|segment| !segment.is_empty())
        .unwrap_or_else(|| "api".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_params_support_both_segment_styles() {
        assert_eq!(
            route_params("/users/{id}/posts/{post_id}"),
            ["id", "post_id"]
        );
        assert_eq!(route_params("/users/:id"), ["id"]);
        assert_eq!(route_params("/static/path"), Vec::<String>::new());
    }

    #[test]
    fn default_ts_module_uses_first_static_segment() {
        assert_eq!(default_ts_module("/users/{id}"), "users");
        assert_eq!(default_ts_module("/admin-tools/users"), "adminTools");
        assert_eq!(default_ts_module("/{id}"), "api");
        assert_eq!(default_ts_module("/"), "api");
    }

    #[test]
    fn doc_strings_join_lines() {
        let item: syn::ItemStruct = syn::parse_quote! {
            /// First line.
            /// Second line.
            struct S;
        };
        assert_eq!(
            doc_string(&item.attrs).as_deref(),
            Some("First line.\nSecond line.")
        );
        let bare: syn::ItemStruct = syn::parse_quote!(
            struct T;
        );
        assert_eq!(doc_string(&bare.attrs), None);
    }
}
