//! Proc macros for typeweld API declarations.
//!
//! The macros are deliberately thin: every validation rule lives in
//! `typeweld-syntax`, shared with the static extractor, so the set of
//! programs that compile and the set of programs that extract are the same.
//! The emitted code is runtime-only — Axum handler adapters, `ApiBound`
//! marker impls with per-field assertions, and `ApiStatus` impls. No contract
//! metadata is generated; the extractor reads it from source.
//!
//! Generated code refers to runtime items through `::typeweld::__rt::*`, so
//! user crates only need a dependency on the `typeweld` facade.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parse_macro_input, parse_quote, visit_mut, Data, DeriveInput, Expr, ExprPath, Fields, ItemFn,
    PathArguments,
};

use typeweld_syntax::{
    analyze_endpoint, reject_generics, ApiAttr, ApiMethod, EndpointSig, FieldWireShape,
    ResponseKind, SerdeAttrs, StatusAttr,
};

/// Marks a type as part of the API contract.
///
/// Validates the restricted serde subset and emits an `ApiBound` impl plus
/// per-field `ApiBound` assertions that guarantee every referenced type is
/// also visible to the static extractor.
#[proc_macro_derive(Api, attributes(serde))]
pub fn derive_api(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    expand_api(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Marks an enum as an API error contract with per-variant HTTP statuses
/// declared as `#[status(404)]`.
#[proc_macro_derive(ApiError, attributes(status, serde))]
pub fn derive_api_error(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    expand_api_error(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Declares an Axum handler as an API endpoint:
/// `#[api(get, "/users/{id}")]`.
#[proc_macro_attribute]
pub fn api(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = match ApiAttr::parse_tokens(args.into()) {
        Ok(args) => args,
        Err(error) => return error.into_compile_error().into(),
    };
    let input = parse_macro_input!(input as ItemFn);

    expand_api_endpoint(&args, &input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Marks a function building an `ApiRouter`; rewrites `.endpoint(handler)`
/// calls to the generated mount helpers.
#[proc_macro_attribute]
pub fn api_router(args: TokenStream, input: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[api_router] does not accept arguments",
        )
        .into_compile_error()
        .into();
    }
    let input = parse_macro_input!(input as ItemFn);

    expand_api_router(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

// ===== derive(Api) =====

fn expand_api(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    reject_generics(&input.generics, "Api types")?;
    let container_serde = SerdeAttrs::from_attrs(&input.attrs)?;
    let ident = &input.ident;
    let mut assertions = Vec::new();

    match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => {
                container_serde.validate_struct_container("Api", ident)?;

                if container_serde.transparent {
                    if fields.named.len() != 1 {
                        return Err(syn::Error::new_spanned(
                            ident,
                            "serde transparent Api structs must contain exactly one field",
                        ));
                    }
                    let field = fields.named.first().expect("length checked");
                    let field_serde = SerdeAttrs::from_attrs(&field.attrs)?;
                    field_serde.validate_transparent_field("Api", field)?;
                    assertions.push(bound_assertion(&field.ty));
                } else {
                    collect_field_assertions("Api", fields.named.iter(), &mut assertions)?;
                }
            }
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                container_serde.validate_newtype_container("Api", ident)?;
                let field = fields.unnamed.first().expect("length checked");
                assertions.push(bound_assertion(&field.ty));
            }
            Fields::Unnamed(_) | Fields::Unit => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "Api tuple structs must contain exactly one field",
                ));
            }
        },
        Data::Enum(data) => {
            container_serde.validate_enum_container("Api", ident)?;
            for variant in &data.variants {
                let variant_serde = SerdeAttrs::from_attrs(&variant.attrs)?;
                variant_serde.validate_enum_variant("Api", variant)?;
                match &variant.fields {
                    Fields::Unit => {}
                    Fields::Named(fields) => {
                        collect_field_assertions("Api", fields.named.iter(), &mut assertions)?;
                    }
                    Fields::Unnamed(_) => {
                        return Err(syn::Error::new_spanned(
                            variant,
                            "Api enum variants must be unit or named-field variants",
                        ));
                    }
                }
            }
        }
        Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                ident,
                "Api cannot be derived for unions",
            ));
        }
    }

    Ok(quote! {
        impl ::typeweld::__rt::ApiBound for #ident {}

        const _: () = {
            fn __typeweld_assert_api_bound<T: ?Sized + ::typeweld::__rt::ApiBound>() {}
            #[allow(dead_code)]
            fn __typeweld_assert_bounds() {
                #(#assertions)*
            }
        };
    })
}

// ===== derive(ApiError) =====

fn expand_api_error(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    reject_generics(&input.generics, "ApiError enums")?;
    let ident = &input.ident;
    let container_serde = SerdeAttrs::from_attrs(&input.attrs)?;

    let Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            ident,
            "ApiError can only be derived for enums",
        ));
    };
    container_serde.validate_enum_container("ApiError", ident)?;

    let mut assertions = Vec::new();
    let mut status_arms = Vec::new();

    for variant in &data.variants {
        let status = StatusAttr::from_variant(variant)?.status;
        let variant_serde = SerdeAttrs::from_attrs(&variant.attrs)?;
        variant_serde.validate_enum_variant("ApiError", variant)?;

        let variant_ident = &variant.ident;
        let pattern = match &variant.fields {
            Fields::Unit => quote!(Self::#variant_ident),
            Fields::Named(fields) => {
                collect_field_assertions("ApiError", fields.named.iter(), &mut assertions)?;
                quote!(Self::#variant_ident { .. })
            }
            Fields::Unnamed(_) => {
                return Err(syn::Error::new_spanned(
                    variant,
                    "ApiError variants must be unit or named-field variants",
                ));
            }
        };
        status_arms.push(quote!(#pattern => #status));
    }

    Ok(quote! {
        impl ::typeweld::__rt::ApiBound for #ident {}

        impl ::typeweld::__rt::ApiStatus for #ident {
            fn status(&self) -> u16 {
                match self {
                    #(#status_arms,)*
                }
            }
        }

        const _: () = {
            fn __typeweld_assert_api_bound<T: ?Sized + ::typeweld::__rt::ApiBound>() {}
            #[allow(dead_code)]
            fn __typeweld_assert_bounds() {
                #(#assertions)*
            }
        };
    })
}

fn collect_field_assertions<'a>(
    derive: &str,
    fields: impl Iterator<Item = &'a syn::Field>,
    assertions: &mut Vec<proc_macro2::TokenStream>,
) -> syn::Result<()> {
    for field in fields {
        let serde = SerdeAttrs::from_attrs(&field.attrs)?;
        serde.validate_field(derive, field)?;
        // Validates the Option / skip_serializing_if pairing.
        let _ = FieldWireShape::from_type(&field.ty, &serde)?;
        assertions.push(bound_assertion(&field.ty));
    }
    Ok(())
}

fn bound_assertion(ty: &syn::Type) -> proc_macro2::TokenStream {
    quote! {
        __typeweld_assert_api_bound::<#ty>();
    }
}

// ===== #[api] =====

fn expand_api_endpoint(args: &ApiAttr, input: &ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    let sig = analyze_endpoint(args, &input.sig)?;

    let fn_ident = &input.sig.ident;
    let handler_ident = format_ident!("__typeweld_handler_{fn_ident}");
    let mount_ident = format_ident!("__typeweld_mount_{fn_ident}");
    let method_ident = match args.method {
        ApiMethod::Get | ApiMethod::Sse => quote!(Get),
        ApiMethod::Post => quote!(Post),
        ApiMethod::Put => quote!(Put),
        ApiMethod::Patch => quote!(Patch),
        ApiMethod::Delete => quote!(Delete),
    };
    let path = &args.path;

    let adapter_inputs = &input.sig.inputs;
    let call_args: Vec<proc_macro2::TokenStream> = input
        .sig
        .inputs
        .iter()
        .map(|input| {
            let syn::FnArg::Typed(input) = input else {
                unreachable!("receivers rejected by analyze_endpoint");
            };
            let syn::Pat::Ident(pat) = input.pat.as_ref() else {
                unreachable!("non-ident patterns rejected by analyze_endpoint");
            };
            let ident = &pat.ident;
            quote!(#ident)
        })
        .collect();
    let endpoint_call = if input.sig.asyncness.is_some() {
        quote!(#fn_ident(#(#call_args),*).await)
    } else {
        quote!(#fn_ident(#(#call_args),*))
    };
    let adapter_body = if sig.error.is_some() {
        quote!(::typeweld::__rt::success_or_error_response(#endpoint_call))
    } else {
        quote!(::typeweld::__rt::into_api_response(#endpoint_call))
    };

    let assertions = endpoint_assertions(&sig);

    Ok(quote! {
        #input

        #[doc(hidden)]
        #[allow(non_snake_case, clippy::unused_async)]
        pub async fn #handler_ident(#adapter_inputs) -> ::typeweld::__rt::Response {
            #adapter_body
        }

        #[doc(hidden)]
        #[allow(non_snake_case)]
        #[must_use]
        pub fn #mount_ident<S>() -> ::typeweld::__rt::EndpointMount<S>
        where
            S: ::core::clone::Clone + ::core::marker::Send + ::core::marker::Sync + 'static,
        {
            ::typeweld::__rt::EndpointMount {
                path: #path,
                method_router: ::typeweld::__rt::method_router(
                    ::typeweld::__rt::HttpMethod::#method_ident,
                    #handler_ident,
                ),
            }
        }

        const _: () = {
            fn __typeweld_assert_api_bound<T: ?Sized + ::typeweld::__rt::ApiBound>() {}
            #[allow(dead_code)]
            fn __typeweld_assert_endpoint() {
                #(#assertions)*
            }
        };
    })
}

fn endpoint_assertions(sig: &EndpointSig<'_>) -> Vec<proc_macro2::TokenStream> {
    let mut assertions = Vec::new();
    for param in &sig.params {
        if param.kind == typeweld_syntax::ParamKind::BinaryBody {
            continue;
        }
        let ty = param.inner_ty;
        assertions.push(quote!(__typeweld_assert_api_bound::<#ty>();));
    }
    match &sig.response {
        ResponseKind::Json(ty) | ResponseKind::Created(ty) | ResponseKind::Sse(ty) => {
            assertions.push(quote!(__typeweld_assert_api_bound::<#ty>();));
        }
        ResponseKind::Empty | ResponseKind::Binary => {}
    }
    // The error type's ApiStatus + Serialize bounds are enforced by the
    // generated handler's success_or_error_response call.
    assertions
}

// ===== #[api_router] =====

fn expand_api_router(mut input: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    let mut rewriter = ApiRouterEndpointRewriter { errors: Vec::new() };
    visit_mut::VisitMut::visit_block_mut(&mut rewriter, &mut input.block);
    if let Some(error) = combine_errors(rewriter.errors) {
        return Err(error);
    }

    Ok(quote!(#input))
}

struct ApiRouterEndpointRewriter {
    errors: Vec<syn::Error>,
}

impl visit_mut::VisitMut for ApiRouterEndpointRewriter {
    fn visit_expr_method_call_mut(&mut self, call: &mut syn::ExprMethodCall) {
        visit_mut::visit_expr_method_call_mut(self, call);

        if call.method != "endpoint" {
            return;
        }

        if call.args.len() != 1 {
            self.errors.push(syn::Error::new_spanned(
                &call.args,
                "#[api_router] .endpoint(...) expects exactly one handler path",
            ));
            return;
        }

        let Some(handler) = call.args.first() else {
            return;
        };
        let Expr::Path(ExprPath {
            qself: None, path, ..
        }) = handler
        else {
            self.errors.push(unsupported_endpoint_arg(handler));
            return;
        };
        if path.segments.is_empty()
            || path
                .segments
                .iter()
                .any(|segment| !matches!(segment.arguments, PathArguments::None))
        {
            self.errors.push(unsupported_endpoint_arg(handler));
            return;
        }

        let mut mount_path = path.clone();
        let last = mount_path
            .segments
            .last_mut()
            .expect("validated handler paths contain a last segment");
        last.ident = format_ident!("__typeweld_mount_{}", last.ident);

        call.args.clear();
        call.args.push(parse_quote!(#mount_path()));
    }
}

fn unsupported_endpoint_arg(expr: &Expr) -> syn::Error {
    syn::Error::new_spanned(
        expr,
        "#[api_router] .endpoint(...) only supports a handler path like `get_user` or \
         `users::get_user`",
    )
}

fn combine_errors(errors: Vec<syn::Error>) -> Option<syn::Error> {
    let mut iter = errors.into_iter();
    let mut combined = iter.next()?;
    for error in iter {
        combined.combine(error);
    }
    Some(combined)
}
