//! Proc macros for the Rust API contract.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields, Type};

#[proc_macro_derive(ApiType, attributes(serde))]
pub fn derive_api_type(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    expand_api_type(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_attribute]
pub fn api(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

fn expand_api_type(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let ident = input.ident;
    let rust_name = ident.to_string();

    let shape = match input.data {
        Data::Struct(data) => match data.fields {
            Fields::Named(fields) => {
                let field_defs = named_field_defs(&rust_name, fields.named.iter());

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
                    let variant_fields = match &variant.fields {
                        Fields::Unit => Vec::new(),
                        Fields::Named(fields) => named_field_defs(
                            &format!("{rust_name}::{variant_name}"),
                            fields.named.iter(),
                        ),
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
                            wire_name: #variant_name.to_owned(),
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

fn named_field_defs<'a>(
    owner_name: &str,
    fields: impl Iterator<Item = &'a syn::Field>,
) -> Vec<proc_macro2::TokenStream> {
    fields
        .map(|field| {
            let field_ident = field
                .ident
                .as_ref()
                .expect("named fields always have identifiers");
            let field_name = field_ident.to_string();
            let field_ty = &field.ty;

            field_def(owner_name, &field_name, field_ty)
        })
        .collect()
}

fn field_def(owner_name: &str, field_name: &str, field_ty: &Type) -> proc_macro2::TokenStream {
    quote! {
        ::api_core::ir::Field {
            id: {
                let owner = <Self as ::api_core::ApiType>::rust_path().join("::");
                ::api_core::ir::SymbolId::from_parts("field", &[&owner, #owner_name, #field_name])
            },
            rust_name: #field_name.to_owned(),
            wire_name: #field_name.to_owned(),
            ts_name: #field_name.to_owned(),
            type_ref: <#field_ty as ::api_core::ApiType>::type_ref(),
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
