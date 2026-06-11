//! Parsing for the `#[api(...)]` endpoint attribute and the `#[status(...)]`
//! error-variant attribute.

use proc_macro2::TokenStream;
use syn::parse::{Parse, ParseStream};
use syn::{Attribute, LitInt, LitStr, Token};

/// The endpoint method declared in `#[api(...)]`.
///
/// `sse` endpoints are served over HTTP `GET` but stream server-sent events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Sse,
}

impl ApiMethod {
    /// The HTTP method used on the wire.
    #[must_use]
    pub const fn http_method(self) -> &'static str {
        match self {
            Self::Get | Self::Sse => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }

    /// Whether endpoints with this method may declare a request body.
    #[must_use]
    pub const fn allows_body(self) -> bool {
        matches!(self, Self::Post | Self::Put | Self::Patch)
    }

    /// Whether this is the server-sent events pseudo-method.
    #[must_use]
    pub const fn is_sse(self) -> bool {
        matches!(self, Self::Sse)
    }

    fn parse_ident(ident: &syn::Ident) -> syn::Result<Self> {
        match ident.to_string().as_str() {
            "get" => Ok(Self::Get),
            "post" => Ok(Self::Post),
            "put" => Ok(Self::Put),
            "patch" => Ok(Self::Patch),
            "delete" => Ok(Self::Delete),
            "sse" => Ok(Self::Sse),
            other => Err(syn::Error::new(
                ident.span(),
                format!(
                    "unsupported #[api] method `{other}`; expected one of get, post, put, \
                     patch, delete, sse"
                ),
            )),
        }
    }
}

/// The parsed `#[api(<method>, "<path>" [, allow_unused])]` attribute.
pub struct ApiAttr {
    pub method: ApiMethod,
    pub path: String,
    pub path_span: proc_macro2::Span,
    pub allow_unused: bool,
}

impl Parse for ApiAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let method_ident: syn::Ident = input.parse().map_err(|_| {
            syn::Error::new(
                input.span(),
                "#[api] expects a method and a path, e.g. #[api(get, \"/users/{id}\")]",
            )
        })?;
        let method = ApiMethod::parse_ident(&method_ident)?;
        input.parse::<Token![,]>()?;
        let path_lit: LitStr = input.parse().map_err(|_| {
            syn::Error::new(
                input.span(),
                "#[api] expects a string route path, e.g. #[api(get, \"/users/{id}\")]",
            )
        })?;
        let path = path_lit.value();
        if !path.starts_with('/') {
            return Err(syn::Error::new_spanned(
                &path_lit,
                "#[api] route paths must start with `/`",
            ));
        }

        let mut allow_unused = false;
        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let flag: syn::Ident = input.parse()?;
            if flag == "allow_unused" {
                allow_unused = true;
            } else {
                return Err(syn::Error::new(
                    flag.span(),
                    format!("unsupported #[api] argument `{flag}`"),
                ));
            }
        }

        Ok(Self {
            method,
            path,
            path_span: path_lit.span(),
            allow_unused,
        })
    }
}

impl ApiAttr {
    /// Parses the token stream inside `#[api(...)]`.
    ///
    /// # Errors
    /// Returns an error when the attribute arguments are malformed.
    pub fn parse_tokens(tokens: TokenStream) -> syn::Result<Self> {
        syn::parse2(tokens)
    }

    /// Parses an `#[api(...)]` attribute as found on an item by the extractor.
    ///
    /// # Errors
    /// Returns an error when the attribute arguments are malformed.
    pub fn from_attribute(attr: &Attribute) -> syn::Result<Self> {
        match &attr.meta {
            syn::Meta::List(list) => Self::parse_tokens(list.tokens.clone()),
            _ => Err(syn::Error::new_spanned(
                attr,
                "#[api] expects a method and a path, e.g. #[api(get, \"/users/{id}\")]",
            )),
        }
    }
}

/// The parsed `#[status(NNN)]` attribute on an API error variant.
pub struct StatusAttr {
    pub status: u16,
}

impl StatusAttr {
    /// Reads the required `#[status(NNN)]` attribute from an error variant.
    ///
    /// # Errors
    /// Returns an error when the attribute is missing, malformed, or the
    /// status is outside the 400..=599 range.
    pub fn from_variant(variant: &syn::Variant) -> syn::Result<Self> {
        let mut status = None;

        for attr in variant
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("status"))
        {
            let literal: LitInt = attr.parse_args().map_err(|_| {
                syn::Error::new_spanned(attr, "#[status] expects a status code, e.g. #[status(404)]")
            })?;
            let value = literal.base10_parse::<u16>()?;
            if !(400..=599).contains(&value) {
                return Err(syn::Error::new_spanned(
                    &literal,
                    "ApiError statuses must be in the 400..=599 range",
                ));
            }
            status = Some(value);
        }

        let Some(status) = status else {
            return Err(syn::Error::new_spanned(
                variant,
                "ApiError variants require #[status(...)], e.g. #[status(404)]",
            ));
        };

        Ok(Self { status })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn parses_method_path_and_flags() {
        let attr = ApiAttr::parse_tokens(quote!(get, "/users/{id}")).expect("parse");
        assert_eq!(attr.method, ApiMethod::Get);
        assert_eq!(attr.path, "/users/{id}");
        assert!(!attr.allow_unused);

        let attr = ApiAttr::parse_tokens(quote!(sse, "/events", allow_unused)).expect("parse");
        assert_eq!(attr.method, ApiMethod::Sse);
        assert!(attr.allow_unused);
        assert_eq!(attr.method.http_method(), "GET");
    }

    #[test]
    fn rejects_malformed_attributes() {
        assert!(ApiAttr::parse_tokens(quote!()).is_err());
        assert!(ApiAttr::parse_tokens(quote!(fetch, "/x")).is_err());
        assert!(ApiAttr::parse_tokens(quote!(get, "no-slash")).is_err());
        assert!(ApiAttr::parse_tokens(quote!(get, "/x", frobnicate)).is_err());
        assert!(ApiAttr::parse_tokens(quote!(get)).is_err());
    }

    #[test]
    fn status_attr_requires_error_range() {
        let variant: syn::Variant = syn::parse_quote! {
            #[status(404)]
            NotFound { id: i32 }
        };
        assert_eq!(StatusAttr::from_variant(&variant).expect("parse").status, 404);

        let missing: syn::Variant = syn::parse_quote!(NotFound);
        assert!(StatusAttr::from_variant(&missing).is_err());

        let out_of_range: syn::Variant = syn::parse_quote! {
            #[status(200)]
            Ok
        };
        assert!(StatusAttr::from_variant(&out_of_range).is_err());
    }
}
