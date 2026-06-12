//! The restricted serde attribute subset understood by typeweld.
//!
//! Only attributes whose effect on the wire shape can be determined statically
//! are accepted; everything else is rejected with an explanation so the
//! generated TypeScript schema can never silently drift from the Rust wire
//! format.

use syn::{Attribute, LitStr};

use crate::rename::RenameRule;

/// Parsed serde attributes for a container, variant, or field.
#[allow(clippy::struct_excessive_bools)]
#[derive(Default)]
pub struct SerdeAttrs {
    pub rename: Option<String>,
    pub rename_all: Option<RenameRule>,
    pub rename_all_fields: Option<RenameRule>,
    pub tag: Option<String>,
    pub aliases: Vec<String>,
    pub transparent: bool,
    pub deny_unknown_fields: bool,
    pub default: bool,
    pub skip_serializing_if_option_none: bool,
}

impl SerdeAttrs {
    /// Parses the `#[serde(...)]` attributes, rejecting unsupported ones.
    ///
    /// # Errors
    /// Returns an error for any serde attribute outside the supported subset.
    pub fn from_attrs(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut serde = Self::default();

        for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    if !meta.input.peek(syn::Token![=]) {
                        return Err(meta.error(
                            "serde rename with separate serialize/deserialize names is not \
                             supported by typeweld; use rename = \"...\" for one wire name",
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
                        "serde content is not supported by typeweld; generated API enums use \
                         internally tagged #[serde(tag = \"_tag\")] shapes",
                    ))
                } else if meta.path.is_ident("untagged") {
                    Err(meta.error(
                        "serde untagged enums are not supported by typeweld because generated \
                         schemas require an explicit _tag discriminator",
                    ))
                } else if meta.path.is_ident("flatten") {
                    Err(meta.error(
                        "serde flatten is not supported by typeweld yet; use a named nested DTO \
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
                        "serde skip attributes are not represented in the API contract; remove \
                         skipped fields from the exported DTO or use a separate API DTO",
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
                            "typeweld supports only #[serde(skip_serializing_if = \
                             \"Option::is_none\")] today; other predicates would make the \
                             generated wire shape inaccurate",
                        ))
                    }
                } else if meta.path.is_ident("serialize_with")
                    || meta.path.is_ident("deserialize_with")
                    || meta.path.is_ident("with")
                {
                    Err(meta.error(
                        "custom serde serializers are not supported by typeweld unless an \
                         explicit API wire-shape override is added; use a representable API DTO \
                         or newtype instead",
                    ))
                } else {
                    Err(meta.error("unsupported serde attribute for typeweld API types"))
                }
            })?;
        }

        Ok(serde)
    }

    /// Validates container-level attributes for a named-field struct.
    ///
    /// # Errors
    /// Returns an error when enum-only or field-only attributes are present.
    pub fn validate_struct_container(&self, derive: &str, ident: &syn::Ident) -> syn::Result<()> {
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

    /// Validates container-level attributes for a newtype struct.
    ///
    /// # Errors
    /// Returns an error for attributes that cannot apply to a newtype wrapper.
    pub fn validate_newtype_container(&self, derive: &str, ident: &syn::Ident) -> syn::Result<()> {
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

    /// Validates container-level attributes for an enum.
    ///
    /// # Errors
    /// Returns an error unless the enum is internally tagged with `_tag`.
    pub fn validate_enum_container(&self, derive: &str, ident: &syn::Ident) -> syn::Result<()> {
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

    /// Validates variant-level attributes on an enum variant.
    ///
    /// # Errors
    /// Returns an error for container-only or field-only attributes.
    pub fn validate_enum_variant(&self, derive: &str, variant: &syn::Variant) -> syn::Result<()> {
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

    /// Validates field-level attributes.
    ///
    /// # Errors
    /// Returns an error for container-only attributes on fields.
    pub fn validate_field(&self, derive: &str, field: &syn::Field) -> syn::Result<()> {
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

    /// Validates the single field of a `#[serde(transparent)]` struct.
    ///
    /// # Errors
    /// Returns an error when the field carries any wire-shape attributes.
    pub fn validate_transparent_field(&self, derive: &str, field: &syn::Field) -> syn::Result<()> {
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

/// Wire-level field presence semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireOptionality {
    Required,
    Optional,
    Nullable,
    OptionalNullable,
}

/// The wire shape of a single field: its inner type and presence semantics.
pub struct FieldWireShape<'a> {
    /// The field type with any `Option` wrapper stripped.
    pub inner_ty: &'a syn::Type,
    pub optionality: WireOptionality,
}

impl<'a> FieldWireShape<'a> {
    /// Derives the wire shape from the field type and its serde attributes.
    ///
    /// # Errors
    /// Returns an error when `skip_serializing_if = "Option::is_none"` is used
    /// on a non-`Option` field.
    pub fn from_type(field_ty: &'a syn::Type, serde: &SerdeAttrs) -> syn::Result<Self> {
        let option_inner = crate::endpoint::wrapper_inner(field_ty, "Option");
        if serde.skip_serializing_if_option_none && option_inner.is_none() {
            return Err(syn::Error::new_spanned(
                field_ty,
                "typeweld supports skip_serializing_if = \"Option::is_none\" only on Option<T> \
                 fields",
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
            inner_ty: option_inner.unwrap_or(field_ty),
            optionality,
        })
    }
}
