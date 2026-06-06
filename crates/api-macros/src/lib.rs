//! Proc macros for the Rust API contract.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parse::Parser, parse_macro_input, parse_quote, visit_mut, Attribute, Data, DeriveInput, Expr,
    ExprMethodCall, ExprPath, Fields, FnArg, GenericArgument, ItemFn, LitStr, PathArguments,
    ReturnType, Type,
};

#[proc_macro_derive(ApiType, attributes(serde))]
pub fn derive_api_type(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    expand_api_type(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_derive(ApiError, attributes(api_error, serde))]
pub fn derive_api_error(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    expand_api_error(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_attribute]
pub fn api(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = match ApiAttr::parse_tokens(args.into()) {
        Ok(args) => args,
        Err(error) => return error.into_compile_error().into(),
    };
    let input = parse_macro_input!(input as ItemFn);

    expand_api_endpoint(args, input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

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
    fn visit_expr_method_call_mut(&mut self, call: &mut ExprMethodCall) {
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
        if path
            .segments
            .iter()
            .any(|segment| !matches!(segment.arguments, PathArguments::None))
        {
            self.errors.push(unsupported_endpoint_arg(handler));
            return;
        }

        if path.segments.is_empty() {
            self.errors.push(unsupported_endpoint_arg(handler));
            return;
        }
        let descriptor_path = generated_router_helper_path(path, "__api_endpoint_descriptor_");
        let adapter_path = generated_router_helper_path(path, "__api_axum_handler_");
        let source = router_mount_source_range();

        call.method = format_ident!("endpoint_descriptor");
        call.args.clear();
        call.args.push(parse_quote!(#descriptor_path()));
        call.args.push(parse_quote!(#adapter_path));
        call.args.push(source);
    }
}

fn generated_router_helper_path(path: &syn::Path, prefix: &str) -> syn::Path {
    let mut helper = path.clone();
    let last = helper
        .segments
        .last_mut()
        .expect("validated handler paths contain a last segment");
    let handler_ident = last.ident.clone();
    last.ident = format_ident!("{prefix}{handler_ident}");
    helper
}

fn router_mount_source_range() -> Expr {
    parse_quote! {
        ::api_core::ir::SourceRange {
            file: file!().to_owned(),
            start_line: line!(),
            start_column: column!(),
            end_line: line!(),
            end_column: column!(),
            full_range: None,
        }
    }
}

fn unsupported_endpoint_arg(expr: &Expr) -> syn::Error {
    syn::Error::new_spanned(
        expr,
        "#[api_router] .endpoint(...) only supports a handler path like `get_user` or `admin::get_user`",
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

fn expand_api_type(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let ident = input.ident;
    let rust_name = ident.to_string();
    let container_serde = SerdeAttrs::from_attrs(&input.attrs)?;
    let mut register_types = Vec::new();

    let shape = match input.data {
        Data::Struct(data) => match data.fields {
            Fields::Named(fields) => {
                container_serde.validate_struct_container("ApiType", &ident)?;

                if container_serde.transparent {
                    if fields.named.len() != 1 {
                        return Err(syn::Error::new_spanned(
                            ident,
                            "serde transparent ApiType structs must contain exactly one field",
                        ));
                    }

                    let field = fields.named.first().expect("length checked");
                    let field_serde = SerdeAttrs::from_attrs(&field.attrs)?;
                    field_serde.validate_transparent_field("ApiType", field)?;
                    let field_ty = &field.ty;
                    register_types.push(register_type_call(field_ty));

                    quote! {
                        ::api_core::ir::TypeShape::Newtype(Box::new(
                            <#field_ty as ::api_core::ApiType>::type_ref()
                        ))
                    }
                } else {
                    let field_metadata = named_field_metadata(
                        "ApiType",
                        &rust_name,
                        fields.named.iter(),
                        container_serde.rename_all,
                    )?;
                    let field_defs = field_metadata.defs;
                    register_types.extend(field_metadata.register_types);

                    quote! {
                        ::api_core::ir::TypeShape::Struct(::api_core::ir::StructShape {
                            fields: vec![#(#field_defs),*],
                        })
                    }
                }
            }
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                container_serde.validate_newtype_container("ApiType", &ident)?;
                let field_ty = &fields.unnamed.first().expect("length checked").ty;
                register_types.push(register_type_call(field_ty));

                quote! {
                    ::api_core::ir::TypeShape::Newtype(Box::new(
                        <#field_ty as ::api_core::ApiType>::type_ref()
                    ))
                }
            }
            Fields::Unnamed(_) | Fields::Unit => {
                return Err(syn::Error::new_spanned(
                    ident,
                    "ApiType tuple structs must contain exactly one field",
                ));
            }
        },
        Data::Enum(data) => {
            container_serde.validate_enum_container("ApiType", &ident)?;
            let variants = data
                .variants
                .iter()
                .map(|variant| {
                    let variant_ident = &variant.ident;
                    let variant_name = variant_ident.to_string();
                    let variant_serde = SerdeAttrs::from_attrs(&variant.attrs)?;
                    variant_serde.validate_enum_variant("ApiType", variant)?;
                    let variant_field_rename_all = variant_serde
                        .rename_all
                        .or(container_serde.rename_all_fields);
                    let wire_name = variant_serde.rename.clone().unwrap_or_else(|| {
                        apply_rename_rule(&variant_name, container_serde.rename_all)
                    });
                    let variant_fields = match &variant.fields {
                        Fields::Unit => Vec::new(),
                        Fields::Named(fields) => {
                            let field_metadata = named_field_metadata(
                                "ApiType",
                                &format!("{rust_name}::{variant_name}"),
                                fields.named.iter(),
                                variant_field_rename_all,
                            )?;
                            register_types.extend(field_metadata.register_types);
                            field_metadata.defs
                        }
                        Fields::Unnamed(_) => {
                            return Err(syn::Error::new_spanned(
                                variant,
                                "ApiType enum variants must be unit or named-field variants",
                            ));
                        }
                    };

                    Ok(quote! {
                        ::api_core::ir::EnumVariant {
                            id: {
                                let owner = <Self as ::api_core::ApiType>::rust_path().join("::");
                                ::api_core::ir::SymbolId::from_parts(
                                    "enum_variant",
                                    &[&owner, #variant_name],
                                )
                            },
                            rust_name: #variant_name.to_owned(),
                            wire_name: #wire_name.to_owned(),
                            fields: vec![#(#variant_fields),*],
                            source: ::api_core::ir::SourceRange {
                                file: file!().to_owned(),
                                start_line: line!(),
                                start_column: column!(),
                                end_line: line!(),
                                end_column: column!(),
                                full_range: None,
                            },
                        }
                    })
                })
                .collect::<syn::Result<Vec<_>>>()?;

            quote! {
                ::api_core::ir::TypeShape::Enum(::api_core::ir::EnumShape {
                    variants: vec![#(#variants),*],
                })
            }
        }
        Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                ident,
                "ApiType cannot be derived for unions",
            ));
        }
    };

    Ok(quote! {
        impl ::api_core::ApiType for #ident {
            const RUST_NAME: &'static str = #rust_name;

            fn rust_path() -> Vec<String> {
                let mut path = module_path!()
                    .split("::")
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                path.push(<Self as ::api_core::ApiType>::RUST_NAME.to_owned());
                path
            }

            fn type_ref() -> ::api_core::ir::TypeRef {
                let rust_path = <Self as ::api_core::ApiType>::rust_path();
                let parts = rust_path.iter().map(String::as_str).collect::<Vec<_>>();

                ::api_core::ir::TypeRef {
                    id: ::api_core::ir::SymbolId::from_parts("type", &parts),
                    name: <Self as ::api_core::ApiType>::TS_NAME.to_owned(),
                }
            }

            fn type_def() -> ::api_core::ir::TypeDef {
                ::api_core::ir::TypeDef {
                    id: <Self as ::api_core::ApiType>::type_ref().id,
                    rust_path: <Self as ::api_core::ApiType>::rust_path(),
                    rust_name: <Self as ::api_core::ApiType>::RUST_NAME.to_owned(),
                    ts_name: <Self as ::api_core::ApiType>::TS_NAME.to_owned(),
                    shape: #shape,
                    source: ::api_core::ir::SourceRange {
                        file: file!().to_owned(),
                        start_line: line!(),
                        start_column: column!(),
                        end_line: line!(),
                        end_column: column!(),
                        full_range: None,
                    },
                }
            }

            fn register_types(registry: &mut ::api_core::TypeRegistry) {
                if registry.insert(<Self as ::api_core::ApiType>::type_def()) {
                    #(#register_types)*
                }
            }
        }
    })
}

fn expand_api_error(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let ident = input.ident;
    let rust_name = ident.to_string();
    let container_serde = SerdeAttrs::from_attrs(&input.attrs)?;

    let Data::Enum(data) = input.data else {
        return Err(syn::Error::new_spanned(
            &ident,
            "ApiError can only be derived for enums",
        ));
    };
    container_serde.validate_enum_container("ApiError", &ident)?;

    let mut api_type_variants = Vec::new();
    let mut error_variants = Vec::new();
    let mut status_arms = Vec::new();
    let mut register_types = Vec::new();

    for variant in &data.variants {
        let variant_ident = &variant.ident;
        let variant_name = variant_ident.to_string();
        let status = ErrorAttrs::from_attrs(&variant.attrs, variant)?.status;
        let variant_serde = SerdeAttrs::from_attrs(&variant.attrs)?;
        variant_serde.validate_enum_variant("ApiError", variant)?;
        let variant_field_rename_all = variant_serde
            .rename_all
            .or(container_serde.rename_all_fields);
        let tag = variant_serde
            .rename
            .unwrap_or_else(|| apply_rename_rule(&variant_name, container_serde.rename_all));
        let fields = match &variant.fields {
            Fields::Unit => Vec::new(),
            Fields::Named(fields) => {
                let field_metadata = named_field_metadata(
                    "ApiError",
                    &format!("{rust_name}::{variant_name}"),
                    fields.named.iter(),
                    variant_field_rename_all,
                )?;
                register_types.extend(field_metadata.register_types);
                field_metadata.defs
            }
            Fields::Unnamed(_) => {
                return Err(syn::Error::new_spanned(
                    variant,
                    "ApiError variants must be unit or named-field variants",
                ));
            }
        };

        api_type_variants.push(quote! {
            ::api_core::ir::EnumVariant {
                id: {
                    let owner = <Self as ::api_core::ApiType>::rust_path().join("::");
                    ::api_core::ir::SymbolId::from_parts("enum_variant", &[&owner, #variant_name])
                },
                rust_name: #variant_name.to_owned(),
                wire_name: #tag.to_owned(),
                fields: vec![#(#fields),*],
                source: ::api_core::ir::SourceRange {
                    file: file!().to_owned(),
                    start_line: line!(),
                    start_column: column!(),
                    end_line: line!(),
                    end_column: column!(),
                    full_range: None,
                },
            }
        });

        error_variants.push(quote! {
            ::api_core::ir::ErrorVariant {
                id: {
                    let owner = <Self as ::api_core::ApiType>::rust_path().join("::");
                    ::api_core::ir::SymbolId::from_parts("error_variant", &[&owner, #variant_name])
                },
                rust_name: #variant_name.to_owned(),
                tag: #tag.to_owned(),
                status: ::api_core::ir::HttpStatus(#status),
                fields: vec![#(#fields),*],
                source: ::api_core::ir::SourceRange {
                    file: file!().to_owned(),
                    start_line: line!(),
                    start_column: column!(),
                    end_line: line!(),
                    end_column: column!(),
                    full_range: None,
                },
            }
        });

        let pattern = match &variant.fields {
            Fields::Unit => quote!(Self::#variant_ident),
            Fields::Named(_) => quote!(Self::#variant_ident { .. }),
            Fields::Unnamed(_) => unreachable!("unnamed variants rejected above"),
        };
        status_arms.push(quote!(#pattern => ::api_core::ir::HttpStatus(#status)));
    }

    Ok(quote! {
        impl ::api_core::ApiType for #ident {
            const RUST_NAME: &'static str = #rust_name;

            fn rust_path() -> Vec<String> {
                let mut path = module_path!()
                    .split("::")
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                path.push(<Self as ::api_core::ApiType>::RUST_NAME.to_owned());
                path
            }

            fn type_ref() -> ::api_core::ir::TypeRef {
                let rust_path = <Self as ::api_core::ApiType>::rust_path();
                let parts = rust_path.iter().map(String::as_str).collect::<Vec<_>>();

                ::api_core::ir::TypeRef {
                    id: ::api_core::ir::SymbolId::from_parts("type", &parts),
                    name: <Self as ::api_core::ApiType>::TS_NAME.to_owned(),
                }
            }

            fn type_def() -> ::api_core::ir::TypeDef {
                ::api_core::ir::TypeDef {
                    id: <Self as ::api_core::ApiType>::type_ref().id,
                    rust_path: <Self as ::api_core::ApiType>::rust_path(),
                    rust_name: <Self as ::api_core::ApiType>::RUST_NAME.to_owned(),
                    ts_name: <Self as ::api_core::ApiType>::TS_NAME.to_owned(),
                    shape: ::api_core::ir::TypeShape::Enum(::api_core::ir::EnumShape {
                        variants: vec![#(#api_type_variants),*],
                    }),
                    source: ::api_core::ir::SourceRange {
                        file: file!().to_owned(),
                        start_line: line!(),
                        start_column: column!(),
                        end_line: line!(),
                        end_column: column!(),
                        full_range: None,
                    },
                }
            }

            fn register_types(registry: &mut ::api_core::TypeRegistry) {
                if registry.insert(<Self as ::api_core::ApiType>::type_def()) {
                    #(#register_types)*
                }
            }
        }

        impl ::api_core::ApiError for #ident {
            fn status(&self) -> ::api_core::ir::HttpStatus {
                match self {
                    #(#status_arms,)*
                }
            }

            fn error_def() -> ::api_core::ir::ErrorDef {
                ::api_core::ir::ErrorDef {
                    id: <Self as ::api_core::ApiError>::error_ref().id,
                    rust_path: <Self as ::api_core::ApiType>::rust_path(),
                    rust_name: <Self as ::api_core::ApiType>::RUST_NAME.to_owned(),
                    ts_name: <Self as ::api_core::ApiType>::TS_NAME.to_owned(),
                    variants: vec![#(#error_variants),*],
                    source: ::api_core::ir::SourceRange {
                        file: file!().to_owned(),
                        start_line: line!(),
                        start_column: column!(),
                        end_line: line!(),
                        end_column: column!(),
                        full_range: None,
                    },
                }
            }

            fn register_error(registry: &mut ::api_core::ContractRegistry) {
                if registry.insert_error(<Self as ::api_core::ApiError>::error_def()) {
                    let registry = registry.types_mut();
                    #(#register_types)*
                }
            }
        }
    })
}

fn expand_api_endpoint(args: ApiAttr, input: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    if input.sig.asyncness.is_none() && !args.is_sse() {
        return Err(syn::Error::new_spanned(
            &input.sig.fn_token,
            "#[api] endpoints must be async functions",
        ));
    }

    let fn_ident = &input.sig.ident;
    let register_ident = format_ident!("__api_register_endpoint_{fn_ident}");
    let descriptor_ident = format_ident!("__api_endpoint_descriptor_{fn_ident}");
    let axum_handler_ident = format_ident!("__api_axum_handler_{fn_ident}");
    let method = args.method_tokens()?;
    let route_params = route_params(&args.path);
    let request = endpoint_request(&input.sig.inputs, &route_params)?;
    let endpoint_return = endpoint_return(&input.sig.output)?;
    args.validate_request_policy(&request, &input.sig)?;
    args.validate_return_policy(&request, &endpoint_return, &input.sig.output)?;
    let transport = args.transport_tokens(&request, &endpoint_return);
    let adapter_inputs = input.sig.inputs.clone();
    let adapter_call_args = endpoint_call_args(&input.sig.inputs)?;
    let adapter_response = endpoint_return.adapter_response(
        fn_ident,
        adapter_call_args,
        input.sig.asyncness.is_some(),
    );
    let request_shape = request.shape;
    let request_registrations = request.registrations;
    let response = endpoint_return.response;
    let errors = endpoint_return.errors;
    let return_registrations = endpoint_return.registrations;
    let allow_unused = args.allow_unused;
    let path = args.path;
    let (ts_namespace, ts_function) = default_ts_path(&path, &fn_ident.to_string());

    Ok(quote! {
        #input

        #[doc(hidden)]
        #[allow(non_snake_case)]
        pub fn #descriptor_ident() -> ::api_core::EndpointDescriptor {
            let request = #request_shape;
            let mut rust_path = module_path!()
                .split("::")
                .collect::<Vec<_>>();
            rust_path.push(stringify!(#fn_ident));

            let endpoint = ::api_core::Endpoint::new(#method, #path)
                .named(rust_path)
                .ts_path([#ts_namespace, #ts_function])
                .transport(#transport)
                .request(request)
                .response(#response)
                .errors(vec![#(#errors),*])
                .allow_unused(#allow_unused);

            ::api_core::EndpointDescriptor::new(endpoint, #register_ident)
        }

        #[doc(hidden)]
        #[allow(non_snake_case, unused_variables)]
        pub fn #register_ident(registry: &mut ::api_core::ContractRegistry) {
            let registry = registry;
            #(#request_registrations)*
            #(#return_registrations)*
        }

        #[doc(hidden)]
        #[allow(non_snake_case)]
        pub async fn #axum_handler_ident(
            #adapter_inputs
        ) -> ::api_axum::Response {
            #adapter_response
        }
    })
}

fn endpoint_call_args(
    inputs: &syn::punctuated::Punctuated<FnArg, syn::Token![,]>,
) -> syn::Result<Vec<proc_macro2::TokenStream>> {
    inputs
        .iter()
        .map(|input| {
            let FnArg::Typed(input) = input else {
                return Err(syn::Error::new_spanned(
                    input,
                    "#[api] endpoints cannot take self receivers",
                ));
            };
            let syn::Pat::Ident(pat_ident) = input.pat.as_ref() else {
                return Err(syn::Error::new_spanned(
                    input,
                    "#[api] endpoint parameters must use simple identifiers",
                ));
            };
            let ident = &pat_ident.ident;
            Ok(quote!(#ident))
        })
        .collect()
}

struct EndpointRequest {
    shape: proc_macro2::TokenStream,
    registrations: Vec<proc_macro2::TokenStream>,
    has_body: bool,
    is_binary_body: bool,
}

fn endpoint_request(
    inputs: &syn::punctuated::Punctuated<FnArg, syn::Token![,]>,
    route_params: &[String],
) -> syn::Result<EndpointRequest> {
    let mut path_fields = Vec::new();
    let mut query_fields = Vec::new();
    let mut body = None;
    let mut body_transport = quote!(::api_core::ir::RequestBodyTransport::Json);
    let mut is_binary_body = false;
    let mut registrations = Vec::new();
    let mut seen_path_params = Vec::new();

    for input in inputs {
        let FnArg::Typed(input) = input else {
            return Err(syn::Error::new_spanned(
                input,
                "#[api] endpoints cannot take self receivers",
            ));
        };
        let syn::Pat::Ident(pat_ident) = input.pat.as_ref() else {
            return Err(syn::Error::new_spanned(
                input,
                "#[api] endpoint parameters must use simple identifiers with Path<T>, Query<T>, or Body<T>",
            ));
        };
        let name = pat_ident.ident.to_string();

        if let Some(inner) = extractor_inner(input.ty.as_ref(), "Path") {
            seen_path_params.push(name.clone());
            path_fields.push(endpoint_field(&name, inner));
            registrations.push(contract_register_type_call(inner));
        } else if let Some(inner) = extractor_inner(input.ty.as_ref(), "Query") {
            query_fields.push(endpoint_field(&name, inner));
            registrations.push(contract_register_type_call(inner));
        } else if let Some(inner) = extractor_inner(input.ty.as_ref(), "Body") {
            ensure_no_endpoint_body(body.is_some(), input)?;
            body = Some(quote!(Some(<#inner as ::api_core::ApiType>::type_ref())));
            registrations.push(contract_register_type_call(inner));
        } else if let Some(inner) = extractor_inner(input.ty.as_ref(), "Binary") {
            ensure_no_endpoint_body(body.is_some(), input)?;
            validate_binary_bytes_type(inner)?;
            body = Some(quote!(Some(<#inner as ::api_core::ApiType>::type_ref())));
            body_transport = quote!(::api_core::ir::RequestBodyTransport::Binary);
            is_binary_body = true;
        } else {
            return Err(syn::Error::new_spanned(
                input.ty.as_ref(),
                "#[api] endpoint parameters must use Path<T>, Query<T>, Body<T>, or Binary<bytes::Bytes> extractors",
            ));
        }
    }

    for route_param in route_params {
        let matches = seen_path_params
            .iter()
            .filter(|seen| *seen == route_param)
            .count();
        if matches == 0 {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("route parameter `{route_param}` requires a matching Path<T> argument"),
            ));
        }
        if matches > 1 {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!(
                    "route parameter `{route_param}` can only have one matching Path<T> argument"
                ),
            ));
        }
    }

    for seen_path_param in &seen_path_params {
        if !route_params
            .iter()
            .any(|route_param| route_param == seen_path_param)
        {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("Path<T> argument `{seen_path_param}` does not match any route parameter"),
            ));
        }
    }

    let has_body = body.is_some();
    let body = body.unwrap_or_else(|| quote!(None));

    Ok(EndpointRequest {
        shape: quote! {
            ::api_core::ir::RequestShape {
                path_params: vec![#(#path_fields),*],
                query_params: vec![#(#query_fields),*],
                body: #body,
                body_transport: #body_transport,
            }
        },
        registrations,
        has_body,
        is_binary_body,
    })
}

fn ensure_no_endpoint_body(has_body: bool, input: &syn::PatType) -> syn::Result<()> {
    if has_body {
        return Err(syn::Error::new_spanned(
            input,
            "#[api] endpoints can only declare one request body extractor",
        ));
    }
    Ok(())
}

fn contract_register_type_call(ty: &Type) -> proc_macro2::TokenStream {
    quote! {
        registry.register_type::<#ty>();
    }
}

fn contract_register_error_call(ty: &Type) -> proc_macro2::TokenStream {
    quote! {
        registry.register_error::<#ty>();
    }
}

fn register_type_call(ty: &Type) -> proc_macro2::TokenStream {
    quote! {
        <#ty as ::api_core::ApiType>::register_types(registry);
    }
}

struct FieldMetadata {
    defs: Vec<proc_macro2::TokenStream>,
    register_types: Vec<proc_macro2::TokenStream>,
}

fn named_field_metadata<'a>(
    derive: &str,
    owner_name: &str,
    fields: impl Iterator<Item = &'a syn::Field>,
    rename_all: Option<RenameRule>,
) -> syn::Result<FieldMetadata> {
    let mut defs = Vec::new();
    let mut register_types = Vec::new();

    for field in fields {
        let field_ident = field
            .ident
            .as_ref()
            .expect("named fields always have identifiers");
        let field_name = field_ident.to_string();
        let field_ty = &field.ty;
        let serde = SerdeAttrs::from_attrs(&field.attrs)?;
        serde.validate_field(derive, field)?;
        let field_shape = FieldWireShape::from_type(field_ty, &serde)?;
        let wire_name = serde
            .rename
            .unwrap_or_else(|| apply_rename_rule(&field_name, rename_all));

        defs.push(field_def(
            owner_name,
            &field_name,
            &wire_name,
            field_shape.type_ref_ty,
            field_shape.optionality,
        ));
        register_types.push(register_type_call(field_shape.type_ref_ty));
    }

    Ok(FieldMetadata {
        defs,
        register_types,
    })
}

fn endpoint_field(name: &str, ty: &Type) -> proc_macro2::TokenStream {
    quote! {
        ::api_core::ir::Field {
            id: {
                let owner = module_path!();
                ::api_core::ir::SymbolId::from_parts("endpoint_field", &[owner, #name])
            },
            rust_name: #name.to_owned(),
            wire_name: #name.to_owned(),
            ts_name: #name.to_owned(),
            type_ref: <#ty as ::api_core::ApiType>::type_ref(),
            optionality: ::api_core::ir::Optionality::Required,
            source: ::api_core::ir::SourceRange {
                file: file!().to_owned(),
                start_line: line!(),
                start_column: column!(),
                end_line: line!(),
                end_column: column!(),
                full_range: None,
            },
        }
    }
}

struct EndpointReturn {
    response: proc_macro2::TokenStream,
    errors: Vec<proc_macro2::TokenStream>,
    registrations: Vec<proc_macro2::TokenStream>,
    is_stream: bool,
    is_binary: bool,
    is_result: bool,
}

fn endpoint_return(output: &ReturnType) -> syn::Result<EndpointReturn> {
    match output {
        ReturnType::Default => Ok(EndpointReturn {
            response: quote!(::api_core::ir::ResponseShape::Empty),
            errors: Vec::new(),
            registrations: Vec::new(),
            is_stream: false,
            is_binary: false,
            is_result: false,
        }),
        ReturnType::Type(_, ty) => return_for_type(ty),
    }
}

fn return_for_type(ty: &Type) -> syn::Result<EndpointReturn> {
    if type_path_last_ident(ty).is_some_and(|ident| ident == "NoContent") {
        return Ok(EndpointReturn {
            response: quote!(::api_core::ir::ResponseShape::Empty),
            errors: Vec::new(),
            registrations: Vec::new(),
            is_stream: false,
            is_binary: false,
            is_result: false,
        });
    }

    if let Some(inner) = extractor_inner(ty, "Binary") {
        validate_binary_bytes_type(inner)?;
        return Ok(EndpointReturn {
            response: quote! {
                ::api_core::ir::ResponseShape::Binary { content_type: None }
            },
            errors: Vec::new(),
            registrations: Vec::new(),
            is_stream: false,
            is_binary: true,
            is_result: false,
        });
    }

    if let Some(inner) = extractor_inner(ty, "Json") {
        return Ok(EndpointReturn {
            response: quote! {
                ::api_core::ir::ResponseShape::Json(<#inner as ::api_core::ApiType>::type_ref())
            },
            errors: Vec::new(),
            registrations: vec![contract_register_type_call(inner)],
            is_stream: false,
            is_binary: false,
            is_result: false,
        });
    }

    if let Some(inner) = extractor_inner(ty, "Created") {
        return Ok(EndpointReturn {
            response: quote! {
                ::api_core::ir::ResponseShape::Created(<#inner as ::api_core::ApiType>::type_ref())
            },
            errors: Vec::new(),
            registrations: vec![contract_register_type_call(inner)],
            is_stream: false,
            is_binary: false,
            is_result: false,
        });
    }

    if let Some(inner) = extractor_inner(ty, "Sse") {
        return Ok(EndpointReturn {
            response: quote! {
                ::api_core::ir::ResponseShape::Stream(<#inner as ::api_core::ApiType>::type_ref())
            },
            errors: Vec::new(),
            registrations: vec![contract_register_type_call(inner)],
            is_stream: true,
            is_binary: false,
            is_result: false,
        });
    }

    if let Some((ok, err)) = result_types(ty) {
        validate_endpoint_error_type(err)?;
        let ok_return = return_for_type(ok)?;
        let mut registrations = ok_return.registrations;
        registrations.push(contract_register_error_call(err));
        return Ok(EndpointReturn {
            response: ok_return.response,
            errors: vec![quote!(<#err as ::api_core::ApiError>::error_ref())],
            registrations,
            is_stream: ok_return.is_stream,
            is_binary: ok_return.is_binary,
            is_result: true,
        });
    }

    Err(syn::Error::new_spanned(
        ty,
        "#[api] endpoint returns must be Json<T>, Created<T>, NoContent, Binary<bytes::Bytes>, or Result of those",
    ))
}

impl EndpointReturn {
    fn adapter_response(
        &self,
        fn_ident: &syn::Ident,
        call_args: Vec<proc_macro2::TokenStream>,
        endpoint_is_async: bool,
    ) -> proc_macro2::TokenStream {
        let endpoint_call = if endpoint_is_async {
            quote!(#fn_ident(#(#call_args),*).await)
        } else {
            quote!(#fn_ident(#(#call_args),*))
        };

        if self.is_result {
            quote!(::api_axum::success_or_error_response(#endpoint_call))
        } else {
            quote!(::api_axum::into_api_response(#endpoint_call))
        }
    }
}

fn validate_endpoint_error_type(err: &Type) -> syn::Result<()> {
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
            "#[api] endpoint errors must be typed ApiError enums; {reason}. Derive ApiError for a \
             domain enum or map the error into one before returning it."
        ),
    )
}

fn is_string_type(ty: &Type) -> bool {
    type_path_last_ident(ty).is_some_and(|ident| ident == "String")
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
            GenericArgument::Type(Type::TraitObject(_)) | GenericArgument::Type(Type::ImplTrait(_))
        )
    })
}

fn extractor_inner<'a>(ty: &'a Type, wrapper: &str) -> Option<&'a Type> {
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

fn route_params(path: &str) -> Vec<String> {
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

fn default_ts_path(path: &str, fn_name: &str) -> (String, String) {
    let namespace = path
        .split('/')
        .find_map(|segment| {
            let segment = segment.trim();
            (!segment.is_empty()
                && !segment.starts_with('{')
                && !segment.starts_with(':')
                && !segment.contains('}'))
            .then(|| to_camel_case(segment))
        })
        .filter(|segment| !segment.is_empty())
        .unwrap_or_else(|| "api".to_owned());

    (namespace, to_camel_case(fn_name))
}

struct ApiAttr {
    method: String,
    path: String,
    allow_unused: bool,
}

impl ApiAttr {
    fn parse_tokens(tokens: proc_macro2::TokenStream) -> syn::Result<Self> {
        let mut method = None;
        let mut path = None;
        let mut allow_unused = false;

        let parser = syn::meta::parser(|meta| {
            if meta.path.is_ident("method") {
                let value: LitStr = meta.value()?.parse()?;
                method = Some(value.value());
                Ok(())
            } else if meta.path.is_ident("path") {
                let value: LitStr = meta.value()?.parse()?;
                path = Some(value.value());
                Ok(())
            } else if meta.path.is_ident("allow_unused") {
                allow_unused = true;
                Ok(())
            } else {
                Err(meta.error("unsupported #[api] argument"))
            }
        });
        parser.parse2(tokens)?;

        let method = method.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                "#[api] requires method = \"...\"",
            )
        })?;
        let path = path.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                "#[api] requires path = \"...\"",
            )
        })?;

        Ok(Self {
            method,
            path,
            allow_unused,
        })
    }

    fn method_tokens(&self) -> syn::Result<proc_macro2::TokenStream> {
        match self.method.as_str() {
            "DELETE" => Ok(quote!(::api_core::ir::HttpMethod::Delete)),
            "GET" => Ok(quote!(::api_core::ir::HttpMethod::Get)),
            "PATCH" => Ok(quote!(::api_core::ir::HttpMethod::Patch)),
            "POST" => Ok(quote!(::api_core::ir::HttpMethod::Post)),
            "PUT" => Ok(quote!(::api_core::ir::HttpMethod::Put)),
            "SSE" => Ok(quote!(::api_core::ir::HttpMethod::Get)),
            _ => Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "unsupported HTTP method for #[api]",
            )),
        }
    }

    fn transport_tokens(
        &self,
        request: &EndpointRequest,
        endpoint_return: &EndpointReturn,
    ) -> proc_macro2::TokenStream {
        if self.is_sse() {
            quote!(::api_core::ir::Transport::ServerSentEvents)
        } else if endpoint_return.is_binary {
            quote!(::api_core::ir::Transport::BinaryDownload)
        } else if request.is_binary_body {
            quote!(::api_core::ir::Transport::BinaryUpload)
        } else {
            quote!(::api_core::ir::Transport::UnaryHttp)
        }
    }

    fn is_sse(&self) -> bool {
        self.method == "SSE"
    }

    fn validate_request_policy(
        &self,
        request: &EndpointRequest,
        signature: &syn::Signature,
    ) -> syn::Result<()> {
        if request.has_body && !self.allows_body() {
            return Err(syn::Error::new_spanned(
                signature,
                format!(
                    "#[api] {} endpoints cannot declare request body extractors; use Query<T> or \
                     change the endpoint method to POST, PUT, or PATCH",
                    self.method
                ),
            ));
        }

        Ok(())
    }

    fn validate_return_policy(
        &self,
        request: &EndpointRequest,
        endpoint_return: &EndpointReturn,
        output: &ReturnType,
    ) -> syn::Result<()> {
        if self.is_sse() && !endpoint_return.is_stream {
            return Err(syn::Error::new_spanned(
                output,
                "#[api(method = \"SSE\")] endpoints must return Sse<T> or Result<Sse<T>, E>",
            ));
        }

        if !self.is_sse() && endpoint_return.is_stream {
            return Err(syn::Error::new_spanned(
                output,
                "#[api] endpoints returning Sse<T> must use method = \"SSE\"",
            ));
        }

        if request.is_binary_body && endpoint_return.is_binary {
            return Err(syn::Error::new_spanned(
                output,
                "#[api] binary upload endpoints cannot also return Binary<bytes::Bytes>; model the response as Json<T>, Created<T>, or NoContent",
            ));
        }

        Ok(())
    }

    fn allows_body(&self) -> bool {
        matches!(self.method.as_str(), "PATCH" | "POST" | "PUT")
    }
}

fn field_def(
    owner_name: &str,
    field_name: &str,
    wire_name: &str,
    field_ty: &Type,
    optionality: WireOptionality,
) -> proc_macro2::TokenStream {
    let optionality = optionality.tokens();

    quote! {
        ::api_core::ir::Field {
            id: {
                let owner = <Self as ::api_core::ApiType>::rust_path().join("::");
                ::api_core::ir::SymbolId::from_parts("field", &[&owner, #owner_name, #field_name])
            },
            rust_name: #field_name.to_owned(),
            wire_name: #wire_name.to_owned(),
            ts_name: #wire_name.to_owned(),
            type_ref: <#field_ty as ::api_core::ApiType>::type_ref(),
            optionality: #optionality,
            source: ::api_core::ir::SourceRange {
                file: file!().to_owned(),
                start_line: line!(),
                start_column: column!(),
                end_line: line!(),
                end_column: column!(),
                full_range: None,
            },
        }
    }
}

#[derive(Clone, Copy)]
enum RenameRule {
    Camel,
    Pascal,
    Snake,
    Kebab,
    ScreamingSnake,
}

impl RenameRule {
    fn parse(value: &str, literal: &LitStr) -> syn::Result<Self> {
        match value {
            "camelCase" => Ok(Self::Camel),
            "PascalCase" => Ok(Self::Pascal),
            "snake_case" => Ok(Self::Snake),
            "kebab-case" => Ok(Self::Kebab),
            "SCREAMING_SNAKE_CASE" => Ok(Self::ScreamingSnake),
            _ => Err(syn::Error::new_spanned(
                literal,
                format!("unsupported serde rename_all rule `{value}`"),
            )),
        }
    }
}

#[derive(Default)]
struct SerdeAttrs {
    rename: Option<String>,
    rename_all: Option<RenameRule>,
    rename_all_fields: Option<RenameRule>,
    tag: Option<String>,
    aliases: Vec<String>,
    transparent: bool,
    deny_unknown_fields: bool,
    default: bool,
    skip_serializing_if_option_none: bool,
}

#[derive(Default)]
struct ErrorAttrs {
    status: Option<u16>,
}

impl ErrorAttrs {
    fn from_attrs(attrs: &[Attribute], variant: &syn::Variant) -> syn::Result<Self> {
        let mut error = Self::default();

        for attr in attrs
            .iter()
            .filter(|attr| attr.path().is_ident("api_error"))
        {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("status") {
                    let value: syn::LitInt = meta.value()?.parse()?;
                    let status = value.base10_parse::<u16>()?;
                    if !(400..=599).contains(&status) {
                        return Err(syn::Error::new_spanned(
                            value,
                            "ApiError statuses must be in the 400..=599 range",
                        ));
                    }
                    error.status = Some(status);
                    Ok(())
                } else {
                    Ok(())
                }
            })?;
        }

        if error.status.is_none() {
            return Err(syn::Error::new_spanned(
                variant,
                "ApiError variants require #[api_error(status = ...)]",
            ));
        }

        Ok(error)
    }
}

impl SerdeAttrs {
    fn from_attrs(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut serde = Self::default();

        for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    if !meta.input.peek(syn::Token![=]) {
                        return Err(meta.error(
                            "serde rename with separate serialize/deserialize names is not \
                             supported by ApiType; use rename = \"...\" for one wire name",
                        ));
                    }
                    let value: LitStr = meta.value()?.parse()?;
                    serde.rename = Some(value.value());
                    Ok(())
                } else if meta.path.is_ident("rename_all") {
                    let value: LitStr = meta.value()?.parse()?;
                    serde.rename_all = Some(RenameRule::parse(&value.value(), &value)?);
                    Ok(())
                } else if meta.path.is_ident("rename_all_fields") {
                    let value: LitStr = meta.value()?.parse()?;
                    serde.rename_all_fields = Some(RenameRule::parse(&value.value(), &value)?);
                    Ok(())
                } else if meta.path.is_ident("tag") {
                    let value: LitStr = meta.value()?.parse()?;
                    serde.tag = Some(value.value());
                    Ok(())
                } else if meta.path.is_ident("content") {
                    Err(meta.error(
                        "serde content is not supported by ApiType; generated API enums use \
                         internally tagged #[serde(tag = \"_tag\")] shapes",
                    ))
                } else if meta.path.is_ident("untagged") {
                    Err(meta.error(
                        "serde untagged enums are not supported by ApiType because generated \
                         schemas require an explicit _tag discriminator",
                    ))
                } else if meta.path.is_ident("flatten") {
                    Err(meta.error(
                        "serde flatten is not supported by ApiType yet; use a named nested DTO \
                         field so the generated schema has an explicit shape",
                    ))
                } else if meta.path.is_ident("default") {
                    serde.default = true;
                    if meta.input.peek(syn::Token![=]) {
                        let _: LitStr = meta.value()?.parse()?;
                    }
                    Ok(())
                } else if meta.path.is_ident("skip")
                    || meta.path.is_ident("skip_serializing")
                    || meta.path.is_ident("skip_deserializing")
                {
                    Err(meta.error(
                        "serde skip attributes are not represented in the API IR; remove skipped \
                         fields from the exported DTO or use a separate API DTO",
                    ))
                } else if meta.path.is_ident("alias") {
                    let value: LitStr = meta.value()?.parse()?;
                    serde.aliases.push(value.value());
                    Ok(())
                } else if meta.path.is_ident("transparent") {
                    serde.transparent = true;
                    Ok(())
                } else if meta.path.is_ident("deny_unknown_fields") {
                    serde.deny_unknown_fields = true;
                    Ok(())
                } else if meta.path.is_ident("skip_serializing_if") {
                    let value: LitStr = meta.value()?.parse()?;
                    if value.value().ends_with("Option::is_none") {
                        serde.skip_serializing_if_option_none = true;
                        Ok(())
                    } else {
                        Err(syn::Error::new_spanned(
                            value,
                            "ApiType supports only #[serde(skip_serializing_if = \
                             \"Option::is_none\")] today; other predicates would make the \
                             generated wire shape inaccurate",
                        ))
                    }
                } else if meta.path.is_ident("serialize_with")
                    || meta.path.is_ident("deserialize_with")
                    || meta.path.is_ident("with")
                {
                    Err(meta.error(
                        "custom serde serializers are not supported by ApiType unless an \
                         explicit API wire-shape override is added; use a representable API DTO \
                         or newtype instead",
                    ))
                } else {
                    Err(meta.error("unsupported serde attribute for ApiType"))
                }
            })?;
        }

        Ok(serde)
    }

    fn validate_struct_container(&self, derive: &str, ident: &syn::Ident) -> syn::Result<()> {
        self.reject_enum_only_attrs(derive, ident)?;
        if self.rename_all_fields.is_some() {
            return Err(syn::Error::new_spanned(
                ident,
                format!(
                    "{derive} supports serde rename_all_fields only on enum containers with \
                     struct variants"
                ),
            ));
        }
        if self.default {
            return Err(syn::Error::new_spanned(
                ident,
                format!("{derive} supports serde default only on fields"),
            ));
        }

        Ok(())
    }

    fn validate_newtype_container(&self, derive: &str, ident: &syn::Ident) -> syn::Result<()> {
        self.reject_enum_only_attrs(derive, ident)?;
        if self.default {
            return Err(syn::Error::new_spanned(
                ident,
                format!("{derive} supports serde default only on fields"),
            ));
        }
        if self.rename_all.is_some() || self.rename_all_fields.is_some() {
            return Err(syn::Error::new_spanned(
                ident,
                format!("{derive} newtypes cannot use serde rename_all attributes"),
            ));
        }

        if self.deny_unknown_fields {
            return Err(syn::Error::new_spanned(
                ident,
                format!(
                    "{derive} newtypes cannot use serde deny_unknown_fields because their wire \
                     shape is the inner value"
                ),
            ));
        }

        Ok(())
    }

    fn validate_enum_container(&self, derive: &str, ident: &syn::Ident) -> syn::Result<()> {
        if self.transparent {
            return Err(syn::Error::new_spanned(
                ident,
                format!("{derive} enums cannot use serde transparent"),
            ));
        }
        if self.default {
            return Err(syn::Error::new_spanned(
                ident,
                format!("{derive} supports serde default only on fields"),
            ));
        }

        if self.deny_unknown_fields {
            return Err(syn::Error::new_spanned(
                ident,
                format!(
                    "{derive} enums cannot use serde deny_unknown_fields; put it on payload DTOs \
                     with generated struct shapes instead"
                ),
            ));
        }

        match self.tag.as_deref() {
            Some("_tag") => Ok(()),
            Some(tag) => Err(syn::Error::new_spanned(
                ident,
                format!(
                    "{derive} enums support only #[serde(tag = \"_tag\")] because generated \
                     TypeScript schemas use _tag discriminators; found tag `{tag}`"
                ),
            )),
            None => Err(syn::Error::new_spanned(
                ident,
                format!(
                    "{derive} enums must use #[serde(tag = \"_tag\")] so Rust JSON and \
                     generated TypeScript schemas share the same discriminator"
                ),
            )),
        }
    }

    fn validate_enum_variant(&self, derive: &str, variant: &syn::Variant) -> syn::Result<()> {
        if self.tag.is_some() {
            return Err(syn::Error::new_spanned(
                variant,
                format!("{derive} supports serde tag only on enum containers"),
            ));
        }
        if self.rename_all_fields.is_some() {
            return Err(syn::Error::new_spanned(
                variant,
                format!("{derive} supports serde rename_all_fields only on enum containers"),
            ));
        }
        if self.default {
            return Err(syn::Error::new_spanned(
                variant,
                format!("{derive} supports serde default only on fields"),
            ));
        }
        if self.transparent {
            return Err(syn::Error::new_spanned(
                variant,
                format!("{derive} enum variants cannot use serde transparent"),
            ));
        }
        if self.deny_unknown_fields {
            return Err(syn::Error::new_spanned(
                variant,
                format!("{derive} enum variants cannot use serde deny_unknown_fields"),
            ));
        }

        Ok(())
    }

    fn validate_field(&self, derive: &str, field: &syn::Field) -> syn::Result<()> {
        if self.tag.is_some() {
            return Err(syn::Error::new_spanned(
                field,
                format!("{derive} fields cannot use serde tag"),
            ));
        }
        if self.rename_all.is_some() || self.rename_all_fields.is_some() {
            return Err(syn::Error::new_spanned(
                field,
                format!("{derive} fields cannot use serde rename_all attributes"),
            ));
        }
        if self.transparent {
            return Err(syn::Error::new_spanned(
                field,
                format!("{derive} fields cannot use serde transparent"),
            ));
        }
        if self.deny_unknown_fields {
            return Err(syn::Error::new_spanned(
                field,
                format!("{derive} fields cannot use serde deny_unknown_fields"),
            ));
        }

        Ok(())
    }

    fn validate_transparent_field(&self, derive: &str, field: &syn::Field) -> syn::Result<()> {
        if self.rename.is_some()
            || self.rename_all.is_some()
            || self.rename_all_fields.is_some()
            || self.tag.is_some()
            || self.transparent
            || self.deny_unknown_fields
            || self.default
            || self.skip_serializing_if_option_none
            || !self.aliases.is_empty()
        {
            return Err(syn::Error::new_spanned(
                field,
                format!(
                    "{derive} serde transparent fields must not carry serde field-shape \
                     attributes because the exported wire shape is the inner value"
                ),
            ));
        }

        Ok(())
    }

    fn reject_enum_only_attrs(&self, derive: &str, ident: &syn::Ident) -> syn::Result<()> {
        if self.tag.is_some() {
            return Err(syn::Error::new_spanned(
                ident,
                format!("{derive} supports serde tag only on enums"),
            ));
        }

        Ok(())
    }
}

struct FieldWireShape<'a> {
    type_ref_ty: &'a Type,
    optionality: WireOptionality,
}

impl<'a> FieldWireShape<'a> {
    fn from_type(field_ty: &'a Type, serde: &SerdeAttrs) -> syn::Result<Self> {
        let option_inner = extractor_inner(field_ty, "Option");
        if serde.skip_serializing_if_option_none && option_inner.is_none() {
            return Err(syn::Error::new_spanned(
                field_ty,
                "ApiType supports skip_serializing_if = \"Option::is_none\" only on Option<T> fields",
            ));
        }

        let optionality = match (
            option_inner.is_some(),
            serde.default || serde.skip_serializing_if_option_none,
        ) {
            (true, true) => WireOptionality::OptionalNullable,
            (true, false) => WireOptionality::Nullable,
            (false, true) => WireOptionality::Optional,
            (false, false) => WireOptionality::Required,
        };

        Ok(Self {
            type_ref_ty: option_inner.unwrap_or(field_ty),
            optionality,
        })
    }
}

#[derive(Clone, Copy)]
enum WireOptionality {
    Required,
    Optional,
    Nullable,
    OptionalNullable,
}

impl WireOptionality {
    fn tokens(self) -> proc_macro2::TokenStream {
        match self {
            Self::Required => quote!(::api_core::ir::Optionality::Required),
            Self::Optional => quote!(::api_core::ir::Optionality::Optional),
            Self::Nullable => quote!(::api_core::ir::Optionality::Nullable),
            Self::OptionalNullable => quote!(::api_core::ir::Optionality::OptionalNullable),
        }
    }
}

fn apply_rename_rule(name: &str, rule: Option<RenameRule>) -> String {
    match rule {
        Some(RenameRule::Camel) => to_camel_case(name),
        Some(RenameRule::Pascal) => split_words(name).join(""),
        Some(RenameRule::Snake) => split_words(name).join("_").to_ascii_lowercase(),
        None => name.to_owned(),
        Some(RenameRule::Kebab) => split_words(name).join("-").to_ascii_lowercase(),
        Some(RenameRule::ScreamingSnake) => split_words(name).join("_").to_ascii_uppercase(),
    }
}

fn to_camel_case(name: &str) -> String {
    let words = split_words(name);
    let Some((first, rest)) = words.split_first() else {
        return String::new();
    };

    let mut output = first.to_ascii_lowercase();
    output.push_str(&rest.join(""));
    output
}

fn split_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();

    for character in name.chars() {
        if character == '_' || character == '-' {
            push_word(&mut words, &mut current);
        } else if character.is_ascii_uppercase() && !current.is_empty() {
            push_word(&mut words, &mut current);
            current.push(character);
        } else {
            current.push(character);
        }
    }

    push_word(&mut words, &mut current);
    words
}

fn push_word(words: &mut Vec<String>, current: &mut String) {
    if current.is_empty() {
        return;
    }

    let mut chars = current.chars();
    let Some(first) = chars.next() else {
        return;
    };
    words.push(format!(
        "{}{}",
        first.to_ascii_uppercase(),
        chars.as_str().to_ascii_lowercase()
    ));
    current.clear();
}

#[cfg(test)]
mod tests {
    use super::{apply_rename_rule, RenameRule};

    #[test]
    fn common_rename_rules_match_serde_spellings() {
        assert_eq!(
            apply_rename_rule("display_name", Some(RenameRule::Camel)),
            "displayName"
        );
        assert_eq!(
            apply_rename_rule("display_name", Some(RenameRule::Pascal)),
            "DisplayName"
        );
        assert_eq!(
            apply_rename_rule("display_name", Some(RenameRule::Kebab)),
            "display-name"
        );
        assert_eq!(
            apply_rename_rule("display_name", Some(RenameRule::ScreamingSnake)),
            "DISPLAY_NAME"
        );
    }
}
