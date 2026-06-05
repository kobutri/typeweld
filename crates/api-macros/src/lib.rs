//! Proc macros for the Rust API contract.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Attribute, Data, DeriveInput, Fields, LitStr, Type};

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
