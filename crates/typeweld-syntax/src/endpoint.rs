//! Endpoint signature analysis shared by the `#[api]` macro and the static
//! extractor.

use syn::{FnArg, GenericArgument, PathArguments, ReturnType, Type};

use crate::api_attr::ApiAttr;
use crate::route_params;

/// How an endpoint parameter reaches the handler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParamKind {
    Path,
    Query,
    Body,
    BinaryBody,
}

/// One analyzed endpoint parameter.
pub struct EndpointParam<'a> {
    pub name: String,
    pub name_span: proc_macro2::Span,
    pub kind: ParamKind,
    /// The type inside the extractor wrapper, e.g. `i32` for `Path<i32>`.
    pub inner_ty: &'a Type,
}

/// The analyzed success channel of an endpoint return type.
pub enum ResponseKind<'a> {
    /// `NoContent`, `()`, or no return type: 204 with an empty body.
    Empty,
    /// `Json<T>`: 200 with a JSON body.
    Json(&'a Type),
    /// `Created<T>`: 201 with a JSON body.
    Created(&'a Type),
    /// `Sse<T, S>`: a server-sent event stream of `T`.
    Sse(&'a Type),
    /// `Binary<bytes::Bytes>`: 200 with an octet-stream body.
    Binary,
}

/// A fully analyzed and validated endpoint signature.
pub struct EndpointSig<'a> {
    pub params: Vec<EndpointParam<'a>>,
    pub response: ResponseKind<'a>,
    /// The domain error type when the endpoint returns a `Result`.
    pub error: Option<&'a Type>,
}

impl EndpointSig<'_> {
    /// Whether the request declares a binary (octet-stream) body.
    #[must_use]
    pub fn has_binary_body(&self) -> bool {
        self.params
            .iter()
            .any(|param| param.kind == ParamKind::BinaryBody)
    }
}

/// Analyzes and validates an `#[api]` endpoint signature.
///
/// All rules enforced here apply identically in the proc macro (as compile
/// errors) and in the static extractor (as diagnostics).
///
/// # Errors
/// Returns an error describing the first rule violation.
pub fn analyze_endpoint<'a>(attr: &ApiAttr, sig: &'a syn::Signature) -> syn::Result<EndpointSig<'a>> {
    if sig.asyncness.is_none() && !attr.method.is_sse() {
        return Err(syn::Error::new_spanned(
            sig.fn_token,
            "#[api] endpoints must be async functions",
        ));
    }
    reject_generics(&sig.generics, "#[api] endpoints")?;

    let params = analyze_params(sig)?;
    validate_route_params(attr, sig, &params)?;
    let (response, error) = analyze_return(&sig.output)?;

    let has_body = params
        .iter()
        .any(|param| matches!(param.kind, ParamKind::Body | ParamKind::BinaryBody));
    if has_body && !attr.method.allows_body() {
        return Err(syn::Error::new_spanned(
            sig,
            format!(
                "#[api] {} endpoints cannot declare request body extractors; use Query<T> or \
                 change the endpoint method to post, put, or patch",
                attr.method.http_method()
            ),
        ));
    }

    let is_binary_body = params
        .iter()
        .any(|param| param.kind == ParamKind::BinaryBody);
    match (&response, attr.method.is_sse()) {
        (ResponseKind::Sse(_), false) => {
            return Err(syn::Error::new_spanned(
                &sig.output,
                "#[api] endpoints returning Sse<T> must use the sse method",
            ));
        }
        (ResponseKind::Sse(_), true) => {}
        (_, true) => {
            return Err(syn::Error::new_spanned(
                &sig.output,
                "#[api(sse, ...)] endpoints must return Sse<T, S> or Result<Sse<T, S>, E>",
            ));
        }
        (_, false) => {}
    }
    if is_binary_body && matches!(response, ResponseKind::Binary) {
        return Err(syn::Error::new_spanned(
            &sig.output,
            "#[api] binary upload endpoints cannot also return Binary<bytes::Bytes>; model the \
             response as Json<T>, Created<T>, or NoContent",
        ));
    }

    Ok(EndpointSig {
        params,
        response,
        error,
    })
}

/// Rejects generic parameters on API items.
///
/// # Errors
/// Returns an error when the item declares type, lifetime, or const params.
pub fn reject_generics(generics: &syn::Generics, what: &str) -> syn::Result<()> {
    if generics.params.is_empty() {
        return Ok(());
    }

    Err(syn::Error::new_spanned(
        generics,
        format!("{what} cannot be generic; use concrete API types"),
    ))
}

fn analyze_params(sig: &syn::Signature) -> syn::Result<Vec<EndpointParam<'_>>> {
    let mut params = Vec::new();
    let mut body_seen = false;
    let mut query_seen = false;

    for input in &sig.inputs {
        let FnArg::Typed(input) = input else {
            return Err(syn::Error::new_spanned(
                input,
                "#[api] endpoints cannot take self receivers",
            ));
        };
        let syn::Pat::Ident(pat_ident) = input.pat.as_ref() else {
            return Err(syn::Error::new_spanned(
                input,
                "#[api] endpoint parameters must use simple identifiers with Path<T>, Query<T>, \
                 Body<T>, or Binary<bytes::Bytes> extractors",
            ));
        };
        let name = pat_ident.ident.to_string();
        let name_span = pat_ident.ident.span();

        let param = if let Some(inner) = wrapper_inner(input.ty.as_ref(), "Path") {
            EndpointParam {
                name,
                name_span,
                kind: ParamKind::Path,
                inner_ty: inner,
            }
        } else if let Some(inner) = wrapper_inner(input.ty.as_ref(), "Query") {
            if query_seen {
                return Err(syn::Error::new_spanned(
                    input,
                    "#[api] endpoints can declare at most one Query<T> extractor; the query \
                     string deserializes into a single struct whose fields become the query \
                     parameters",
                ));
            }
            query_seen = true;
            EndpointParam {
                name,
                name_span,
                kind: ParamKind::Query,
                inner_ty: inner,
            }
        } else if let Some(inner) = wrapper_inner(input.ty.as_ref(), "Body") {
            ensure_single_body(&mut body_seen, input)?;
            EndpointParam {
                name,
                name_span,
                kind: ParamKind::Body,
                inner_ty: inner,
            }
        } else if let Some(inner) = wrapper_inner(input.ty.as_ref(), "Binary") {
            ensure_single_body(&mut body_seen, input)?;
            validate_binary_bytes_type(inner)?;
            EndpointParam {
                name,
                name_span,
                kind: ParamKind::BinaryBody,
                inner_ty: inner,
            }
        } else {
            return Err(syn::Error::new_spanned(
                input.ty.as_ref(),
                "#[api] endpoint parameters must use Path<T>, Query<T>, Body<T>, or \
                 Binary<bytes::Bytes> extractors",
            ));
        };
        params.push(param);
    }

    Ok(params)
}

fn ensure_single_body(body_seen: &mut bool, input: &syn::PatType) -> syn::Result<()> {
    if *body_seen {
        return Err(syn::Error::new_spanned(
            input,
            "#[api] endpoints can only declare one request body extractor",
        ));
    }
    *body_seen = true;
    Ok(())
}

fn validate_route_params(
    attr: &ApiAttr,
    sig: &syn::Signature,
    params: &[EndpointParam<'_>],
) -> syn::Result<()> {
    let route_params = route_params(&attr.path);
    let path_params: Vec<&EndpointParam<'_>> = params
        .iter()
        .filter(|param| param.kind == ParamKind::Path)
        .collect();

    for route_param in &route_params {
        let matches = path_params
            .iter()
            .filter(|param| param.name == *route_param)
            .count();
        if matches == 0 {
            return Err(syn::Error::new(
                attr.path_span,
                format!("route parameter `{route_param}` requires a matching Path<T> argument"),
            ));
        }
        if matches > 1 {
            return Err(syn::Error::new_spanned(
                sig,
                format!(
                    "route parameter `{route_param}` can only have one matching Path<T> argument"
                ),
            ));
        }
    }

    for param in path_params {
        if !route_params.iter().any(|route| route == &param.name) {
            return Err(syn::Error::new(
                param.name_span,
                format!(
                    "Path<T> argument `{}` does not match any route parameter",
                    param.name
                ),
            ));
        }
    }

    Ok(())
}

fn analyze_return(output: &ReturnType) -> syn::Result<(ResponseKind<'_>, Option<&Type>)> {
    match output {
        ReturnType::Default => Ok((ResponseKind::Empty, None)),
        ReturnType::Type(_, ty) => {
            if let Some((ok, err)) = result_types(ty) {
                validate_endpoint_error_type(err)?;
                let response = success_response(ok)?;
                Ok((response, Some(err)))
            } else {
                Ok((success_response(ty)?, None))
            }
        }
    }
}

fn success_response(ty: &Type) -> syn::Result<ResponseKind<'_>> {
    if type_path_last_ident(ty).is_some_and(|ident| ident == "NoContent") || is_unit_type(ty) {
        return Ok(ResponseKind::Empty);
    }
    if let Some(inner) = wrapper_inner(ty, "Binary") {
        validate_binary_bytes_type(inner)?;
        return Ok(ResponseKind::Binary);
    }
    if let Some(inner) = wrapper_inner(ty, "Json") {
        return Ok(ResponseKind::Json(inner));
    }
    if let Some(inner) = wrapper_inner(ty, "Created") {
        return Ok(ResponseKind::Created(inner));
    }
    if let Some(inner) = wrapper_inner(ty, "Sse") {
        return Ok(ResponseKind::Sse(inner));
    }

    Err(syn::Error::new_spanned(
        ty,
        "#[api] endpoint returns must be Json<T>, Created<T>, NoContent, Sse<T, S>, \
         Binary<bytes::Bytes>, or Result of those",
    ))
}

/// Rejects opaque or transport-level error types in endpoint `Result`s.
///
/// # Errors
/// Returns an error explaining why the type cannot form an API error channel.
pub fn validate_endpoint_error_type(err: &Type) -> syn::Result<()> {
    if is_string_type(err) {
        return Err(unsupported_endpoint_error(
            err,
            "`String` errors are not finite or catchable in generated Effect clients",
        ));
    }

    if is_anyhow_error(err) {
        return Err(unsupported_endpoint_error(
            err,
            "`anyhow::Error` is opaque and cannot be represented as a generated error channel",
        ));
    }

    if is_std_io_error(err) {
        return Err(unsupported_endpoint_error(
            err,
            "`std::io::Error` is transport/runtime detail, not a public API error contract",
        ));
    }

    if is_boxed_dyn_error(err) {
        return Err(unsupported_endpoint_error(
            err,
            "`Box<dyn Error>` is opaque and cannot be represented as a generated error channel",
        ));
    }

    if matches!(err, Type::TraitObject(_)) {
        return Err(unsupported_endpoint_error(
            err,
            "`dyn Error` is opaque and cannot be represented as a generated error channel",
        ));
    }

    if matches!(err, Type::ImplTrait(_) | Type::Infer(_)) {
        return Err(unsupported_endpoint_error(
            err,
            "opaque endpoint error types cannot be represented as generated error channels",
        ));
    }

    Ok(())
}

fn unsupported_endpoint_error(err: &Type, reason: &str) -> syn::Error {
    syn::Error::new_spanned(
        err,
        format!(
            "#[api] endpoint errors must be typed ApiError enums; {reason}. Derive ApiError for \
             a domain enum or map the error into one before returning it."
        ),
    )
}

fn validate_binary_bytes_type(ty: &Type) -> syn::Result<()> {
    let Some(segments) = type_path_segments(ty) else {
        return Err(unsupported_binary_type(ty));
    };

    let is_bytes = segments.last().is_some_and(|segment| segment == "Bytes");
    let is_known_bytes_path = segments.len() == 1
        || segments.as_slice() == ["bytes", "Bytes"]
        || segments.as_slice() == ["axum", "body", "Bytes"];

    if is_bytes && is_known_bytes_path {
        return Ok(());
    }

    Err(unsupported_binary_type(ty))
}

fn unsupported_binary_type(ty: &Type) -> syn::Error {
    syn::Error::new_spanned(
        ty,
        "#[api] Binary<T> endpoints currently support bytes::Bytes or axum::body::Bytes only",
    )
}

fn is_string_type(ty: &Type) -> bool {
    type_path_last_ident(ty).is_some_and(|ident| ident == "String")
}

fn is_unit_type(ty: &Type) -> bool {
    matches!(ty, Type::Tuple(tuple) if tuple.elems.is_empty())
}

fn is_anyhow_error(ty: &Type) -> bool {
    type_path_segments(ty).is_some_and(|segments| {
        segments.last().is_some_and(|segment| segment == "Error")
            && segments.iter().any(|segment| segment == "anyhow")
    })
}

fn is_std_io_error(ty: &Type) -> bool {
    type_path_segments(ty).is_some_and(|segments| {
        segments.last().is_some_and(|segment| segment == "Error")
            && segments.windows(2).any(|window| {
                (window[0] == "std" && window[1] == "io")
                    || (window[0] == "tokio" && window[1] == "io")
            })
    })
}

fn is_boxed_dyn_error(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    if segment.ident != "Box" {
        return false;
    }
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return false;
    };
    args.args.iter().any(|arg| {
        matches!(
            arg,
            GenericArgument::Type(Type::TraitObject(_) | Type::ImplTrait(_))
        )
    })
}

/// Returns the first generic argument of a wrapper type like `Path<T>`.
///
/// Matches by the last path segment, so `typeweld::Path<T>` works too.
#[must_use]
pub fn wrapper_inner<'a>(ty: &'a Type, wrapper: &str) -> Option<&'a Type> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != wrapper {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    let GenericArgument::Type(inner) = args.args.first()? else {
        return None;
    };
    Some(inner)
}

fn result_types(ty: &Type) -> Option<(&Type, &Type)> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != "Result" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    let mut args = args.args.iter();
    let Some(GenericArgument::Type(ok)) = args.next() else {
        return None;
    };
    let Some(GenericArgument::Type(err)) = args.next() else {
        return None;
    };
    Some((ok, err))
}

fn type_path_last_ident(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else {
        return None;
    };
    Some(path.path.segments.last()?.ident.to_string())
}

fn type_path_segments(ty: &Type) -> Option<Vec<String>> {
    let Type::Path(path) = ty else {
        return None;
    };
    Some(
        path.path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_attr::ApiAttr;
    use quote::quote;

    fn attr(tokens: proc_macro2::TokenStream) -> ApiAttr {
        ApiAttr::parse_tokens(tokens).expect("attr")
    }

    fn sig(tokens: proc_macro2::TokenStream) -> syn::Signature {
        let item: syn::ItemFn = syn::parse2(tokens).expect("fn");
        item.sig
    }

    #[test]
    fn accepts_a_typical_unary_endpoint() {
        let attr = attr(quote!(get, "/users/{id}"));
        let sig = sig(quote! {
            async fn get_user(id: Path<i32>, filter: Query<Filter>) -> Result<Json<User>, UserError> {}
        });
        let endpoint = analyze_endpoint(&attr, &sig).expect("analyze");
        assert_eq!(endpoint.params.len(), 2);
        assert_eq!(endpoint.params[0].kind, ParamKind::Path);
        assert_eq!(endpoint.params[1].kind, ParamKind::Query);
        assert!(matches!(endpoint.response, ResponseKind::Json(_)));
        assert!(endpoint.error.is_some());
    }

    #[test]
    fn enforces_route_param_matching() {
        let attr = attr(quote!(get, "/users/{id}"));
        let missing = sig(quote! {
            async fn get_user() -> Json<User> {}
        });
        assert!(analyze_endpoint(&attr, &missing).is_err());

        let extra = sig(quote! {
            async fn get_user(id: Path<i32>, other: Path<i32>) -> Json<User> {}
        });
        assert!(analyze_endpoint(&attr, &extra).is_err());
    }

    #[test]
    fn enforces_body_policy_by_method() {
        let get_attr = attr(quote!(get, "/users"));
        let with_body = sig(quote! {
            async fn list(body: Body<Filter>) -> Json<User> {}
        });
        assert!(analyze_endpoint(&get_attr, &with_body).is_err());

        let post_attr = attr(quote!(post, "/users"));
        assert!(analyze_endpoint(&post_attr, &with_body).is_ok());
    }

    #[test]
    fn enforces_sse_pairing_both_directions() {
        let sse_attr = attr(quote!(sse, "/events"));
        let unary = sig(quote! {
            async fn events() -> Json<User> {}
        });
        assert!(analyze_endpoint(&sse_attr, &unary).is_err());

        let stream = sig(quote! {
            fn events() -> Result<Sse<UserEvent, S>, EventError> {}
        });
        assert!(analyze_endpoint(&sse_attr, &stream).is_ok());

        let get_attr = attr(quote!(get, "/events"));
        assert!(analyze_endpoint(&get_attr, &stream).is_err());
    }

    #[test]
    fn rejects_opaque_error_types() {
        let attr = attr(quote!(get, "/x"));
        for err in [
            quote!(String),
            quote!(anyhow::Error),
            quote!(std::io::Error),
            quote!(Box<dyn std::error::Error>),
        ] {
            let sig = sig(quote! {
                async fn x() -> Result<Json<User>, #err> {}
            });
            assert!(analyze_endpoint(&attr, &sig).is_err(), "expected rejection: {err}");
        }
    }

    #[test]
    fn rejects_generics_and_self_receivers() {
        let attr = attr(quote!(get, "/x"));
        let generic = sig(quote! {
            async fn x<T>() -> Json<User> {}
        });
        assert!(analyze_endpoint(&attr, &generic).is_err());

        let receiver = sig(quote! {
            async fn x(&self) -> Json<User> {}
        });
        assert!(analyze_endpoint(&attr, &receiver).is_err());
    }

    #[test]
    fn rejects_binary_upload_with_binary_download() {
        let attr = attr(quote!(post, "/blobs"));
        let sig = sig(quote! {
            async fn roundtrip(data: Binary<bytes::Bytes>) -> Binary<bytes::Bytes> {}
        });
        assert!(analyze_endpoint(&attr, &sig).is_err());
    }
}
