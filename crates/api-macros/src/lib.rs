//! Proc macros for the Rust API contract.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parse::Parser, parse_macro_input, Attribute, Data, DeriveInput, Fields, FnArg, GenericArgument,
    ItemFn, LitStr, PathArguments, ReturnType, Type,
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

fn expand_api_type(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let ident = input.ident;
    let rust_name = ident.to_string();
    let container_serde = SerdeAttrs::from_attrs(&input.attrs)?;

    let shape = match input.data {
        Data::Struct(data) => match data.fields {
            Fields::Named(fields) => {
                let field_defs =
                    named_field_defs(&rust_name, fields.named.iter(), container_serde.rename_all)?;

                quote! {
                    ::api_core::ir::TypeShape::Struct(::api_core::ir::StructShape {
                        fields: vec![#(#field_defs),*],
                    })
                }
            }
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                let field_ty = &fields.unnamed.first().expect("length checked").ty;

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
            let variants = data
                .variants
                .iter()
                .map(|variant| {
                    let variant_ident = &variant.ident;
                    let variant_name = variant_ident.to_string();
                    let variant_serde = SerdeAttrs::from_attrs(&variant.attrs)?;
                    let wire_name = variant_serde.rename.unwrap_or_else(|| {
                        apply_rename_rule(&variant_name, container_serde.rename_all)
                    });
                    let variant_fields = match &variant.fields {
                        Fields::Unit => Vec::new(),
                        Fields::Named(fields) => named_field_defs(
                            &format!("{rust_name}::{variant_name}"),
                            fields.named.iter(),
                            None,
                        )?,
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
                path.push(Self::RUST_NAME.to_owned());
                path
            }

            fn type_ref() -> ::api_core::ir::TypeRef {
                let rust_path = <Self as ::api_core::ApiType>::rust_path();
                let parts = rust_path.iter().map(String::as_str).collect::<Vec<_>>();

                ::api_core::ir::TypeRef {
                    id: ::api_core::ir::SymbolId::from_parts("type", &parts),
                    name: Self::TS_NAME.to_owned(),
                }
            }

            fn type_def() -> ::api_core::ir::TypeDef {
                ::api_core::ir::TypeDef {
                    id: <Self as ::api_core::ApiType>::type_ref().id,
                    rust_path: <Self as ::api_core::ApiType>::rust_path(),
                    rust_name: Self::RUST_NAME.to_owned(),
                    ts_name: Self::TS_NAME.to_owned(),
                    shape: #shape,
                    source: ::api_core::ir::SourceRange {
                        file: file!().to_owned(),
                        start_line: line!(),
                        start_column: column!(),
                        end_line: line!(),
                        end_column: column!(),
                    },
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
            ident,
            "ApiError can only be derived for enums",
        ));
    };

    let mut api_type_variants = Vec::new();
    let mut error_variants = Vec::new();

    for variant in &data.variants {
        let variant_ident = &variant.ident;
        let variant_name = variant_ident.to_string();
        let status = ErrorAttrs::from_attrs(&variant.attrs, variant)?.status;
        let variant_serde = SerdeAttrs::from_attrs(&variant.attrs)?;
        let tag = variant_serde
            .rename
            .unwrap_or_else(|| apply_rename_rule(&variant_name, container_serde.rename_all));
        let fields = match &variant.fields {
            Fields::Unit => Vec::new(),
            Fields::Named(fields) => named_field_defs(
                &format!("{rust_name}::{variant_name}"),
                fields.named.iter(),
                None,
            )?,
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
                },
            }
        });
    }

    Ok(quote! {
        impl ::api_core::ApiType for #ident {
            const RUST_NAME: &'static str = #rust_name;

            fn rust_path() -> Vec<String> {
                let mut path = module_path!()
                    .split("::")
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                path.push(Self::RUST_NAME.to_owned());
                path
            }

            fn type_ref() -> ::api_core::ir::TypeRef {
                let rust_path = <Self as ::api_core::ApiType>::rust_path();
                let parts = rust_path.iter().map(String::as_str).collect::<Vec<_>>();

                ::api_core::ir::TypeRef {
                    id: ::api_core::ir::SymbolId::from_parts("type", &parts),
                    name: Self::TS_NAME.to_owned(),
                }
            }

            fn type_def() -> ::api_core::ir::TypeDef {
                ::api_core::ir::TypeDef {
                    id: <Self as ::api_core::ApiType>::type_ref().id,
                    rust_path: <Self as ::api_core::ApiType>::rust_path(),
                    rust_name: Self::RUST_NAME.to_owned(),
                    ts_name: Self::TS_NAME.to_owned(),
                    shape: ::api_core::ir::TypeShape::Enum(::api_core::ir::EnumShape {
                        variants: vec![#(#api_type_variants),*],
                    }),
                    source: ::api_core::ir::SourceRange {
                        file: file!().to_owned(),
                        start_line: line!(),
                        start_column: column!(),
                        end_line: line!(),
                        end_column: column!(),
                    },
                }
            }
        }

        impl ::api_core::ApiError for #ident {
            fn error_def() -> ::api_core::ir::ErrorDef {
                ::api_core::ir::ErrorDef {
                    id: <Self as ::api_core::ApiError>::error_ref().id,
                    rust_path: <Self as ::api_core::ApiType>::rust_path(),
                    rust_name: Self::RUST_NAME.to_owned(),
                    ts_name: Self::TS_NAME.to_owned(),
                    variants: vec![#(#error_variants),*],
                    source: ::api_core::ir::SourceRange {
                        file: file!().to_owned(),
                        start_line: line!(),
                        start_column: column!(),
                        end_line: line!(),
                        end_column: column!(),
                    },
                }
            }
        }
    })
}

fn expand_api_endpoint(args: ApiAttr, input: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    if input.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            &input.sig.fn_token,
            "#[api] endpoints must be async functions",
        ));
    }

    let fn_ident = &input.sig.ident;
    let metadata_ident = format_ident!("__api_endpoint_{fn_ident}");
    let method = args.method_tokens()?;
    let route_params = route_params(&args.path);
    let request = endpoint_request(&input.sig.inputs, &route_params)?;
    let endpoint_return = endpoint_return(&input.sig.output)?;
    let response = endpoint_return.response;
    let errors = endpoint_return.errors;
    let allow_unused = args.allow_unused;
    let path = args.path;

    Ok(quote! {
        #input

        #[allow(non_snake_case)]
        pub fn #metadata_ident() -> ::api_core::Endpoint {
            let request = #request;
            let mut rust_path = module_path!()
                .split("::")
                .collect::<Vec<_>>();
            rust_path.push(stringify!(#fn_ident));

            ::api_core::Endpoint::new(#method, #path)
                .named(rust_path)
                .request(request)
                .response(#response)
                .errors(vec![#(#errors),*])
                .allow_unused(#allow_unused)
        }
    })
}

fn endpoint_request(
    inputs: &syn::punctuated::Punctuated<FnArg, syn::Token![,]>,
    route_params: &[String],
) -> syn::Result<proc_macro2::TokenStream> {
    let mut path_fields = Vec::new();
    let mut query_fields = Vec::new();
    let mut body = None;
    let mut seen_path_params = Vec::new();

    for input in inputs {
        let FnArg::Typed(input) = input else {
            return Err(syn::Error::new_spanned(
                input,
                "#[api] endpoints cannot take self receivers",
            ));
        };
        let syn::Pat::Ident(pat_ident) = input.pat.as_ref() else {
            continue;
        };
        let name = pat_ident.ident.to_string();

        if let Some(inner) = extractor_inner(input.ty.as_ref(), "Path") {
            seen_path_params.push(name.clone());
            path_fields.push(endpoint_field(&name, inner));
        } else if let Some(inner) = extractor_inner(input.ty.as_ref(), "Query") {
            query_fields.push(endpoint_field(&name, inner));
        } else if let Some(inner) = extractor_inner(input.ty.as_ref(), "Body") {
            if body.is_some() {
                return Err(syn::Error::new_spanned(
                    input,
                    "#[api] endpoints can only declare one Body<T> extractor",
                ));
            }
            body = Some(quote!(Some(<#inner as ::api_core::ApiType>::type_ref())));
        }
    }

    for route_param in route_params {
        if !seen_path_params.iter().any(|seen| seen == route_param) {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("route parameter `{route_param}` requires a matching Path<T> argument"),
            ));
        }
    }

    let body = body.unwrap_or_else(|| quote!(None));

    Ok(quote! {
        ::api_core::ir::RequestShape {
            path_params: vec![#(#path_fields),*],
            query_params: vec![#(#query_fields),*],
            body: #body,
        }
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
            },
        }
    }
}

struct EndpointReturn {
    response: proc_macro2::TokenStream,
    errors: Vec<proc_macro2::TokenStream>,
}

fn endpoint_return(output: &ReturnType) -> syn::Result<EndpointReturn> {
    match output {
        ReturnType::Default => Ok(EndpointReturn {
            response: quote!(::api_core::ir::ResponseShape::Empty),
            errors: Vec::new(),
        }),
        ReturnType::Type(_, ty) => return_for_type(ty),
    }
}

fn return_for_type(ty: &Type) -> syn::Result<EndpointReturn> {
    if type_path_last_ident(ty).is_some_and(|ident| ident == "NoContent") {
        return Ok(EndpointReturn {
            response: quote!(::api_core::ir::ResponseShape::Empty),
            errors: Vec::new(),
        });
    }

    if let Some(inner) = extractor_inner(ty, "Json").or_else(|| extractor_inner(ty, "Created")) {
        return Ok(EndpointReturn {
            response: quote! {
                ::api_core::ir::ResponseShape::Json(<#inner as ::api_core::ApiType>::type_ref())
            },
            errors: Vec::new(),
        });
    }

    if let Some((ok, err)) = result_types(ty) {
        let ok_return = return_for_type(ok)?;
        return Ok(EndpointReturn {
            response: ok_return.response,
            errors: vec![quote!(<#err as ::api_core::ApiError>::error_ref())],
        });
    }

    Err(syn::Error::new_spanned(
        ty,
        "#[api] endpoint returns must be Json<T>, Created<T>, NoContent, or Result of those",
    ))
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
            _ => Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "unsupported HTTP method for #[api]",
            )),
        }
    }
}

fn named_field_defs<'a>(
    owner_name: &str,
    fields: impl Iterator<Item = &'a syn::Field>,
    rename_all: Option<RenameRule>,
) -> syn::Result<Vec<proc_macro2::TokenStream>> {
    fields
        .map(|field| {
            let field_ident = field
                .ident
                .as_ref()
                .expect("named fields always have identifiers");
            let field_name = field_ident.to_string();
            let field_ty = &field.ty;
            let serde = SerdeAttrs::from_attrs(&field.attrs)?;
            let wire_name = serde
                .rename
                .unwrap_or_else(|| apply_rename_rule(&field_name, rename_all));
            let optional = serde.skip_serializing_if_option_none;

            Ok(field_def(
                owner_name,
                &field_name,
                &wire_name,
                field_ty,
                optional,
            ))
        })
        .collect()
}

fn field_def(
    owner_name: &str,
    field_name: &str,
    wire_name: &str,
    field_ty: &Type,
    optional: bool,
) -> proc_macro2::TokenStream {
    let optionality = if optional {
        quote!(::api_core::ir::Optionality::Optional)
    } else {
        quote!(::api_core::ir::Optionality::Required)
    };

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
                    let value: LitStr = meta.value()?.parse()?;
                    serde.rename = Some(value.value());
                    Ok(())
                } else if meta.path.is_ident("rename_all") {
                    let value: LitStr = meta.value()?.parse()?;
                    serde.rename_all = Some(RenameRule::parse(&value.value(), &value)?);
                    Ok(())
                } else if meta.path.is_ident("skip_serializing_if") {
                    let value: LitStr = meta.value()?.parse()?;
                    serde.skip_serializing_if_option_none =
                        value.value().ends_with("Option::is_none");
                    Ok(())
                } else {
                    Ok(())
                }
            })?;
        }

        Ok(serde)
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
