//! A minimal TypeScript AST covering exactly what the generator emits.
//!
//! Declaration names, object keys, and interface fields can carry a
//! [`MarkRef`]; the printer records the exact output range of every marked
//! node, so the Rust-symbol-to-generated-TypeScript map is correct by
//! construction.

use crate::ir::SymbolId;

/// What kind of symbol a mark points back to.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum MarkKind {
    Endpoint,
    Type,
    Error,
    ErrorVariant,
    EnumVariant,
    /// A field's property on a generated interface (the rename anchor).
    Field,
    /// A field's key inside a `Schema.Struct` object literal.
    FieldKey,
    Param,
}

/// Attaches a Rust symbol to a generated node.
#[derive(Clone, Debug)]
pub struct MarkRef {
    pub symbol_id: SymbolId,
    pub kind: MarkKind,
}

/// A top-level item in a generated file.
pub enum Item {
    Import {
        type_only: bool,
        names: Vec<String>,
        from: String,
    },
    ExportStar {
        from: String,
    },
    ExportStarAs {
        alias: String,
        from: String,
    },
    ExportNames {
        names: Vec<String>,
        from: String,
    },
    Const {
        doc: Option<String>,
        name: String,
        mark: Option<MarkRef>,
        /// Optional type annotation, rendered verbatim.
        ty: Option<String>,
        value: Expr,
        as_const: bool,
    },
    /// `export const name = (params): ret => body`
    FnConst {
        doc: Option<String>,
        name: String,
        mark: Option<MarkRef>,
        params: Vec<(String, String)>,
        ret: String,
        body: Expr,
    },
    TypeAlias {
        doc: Option<String>,
        name: String,
        mark: Option<MarkRef>,
        ty: String,
    },
    Interface {
        doc: Option<String>,
        name: String,
        mark: Option<MarkRef>,
        fields: Vec<InterfaceField>,
    },
    /// `export class name extends <expr> {}`
    Class {
        doc: Option<String>,
        name: String,
        mark: Option<MarkRef>,
        extends: Expr,
    },
    /// Verbatim multi-line text, re-indented to the current level.
    Raw(String),
}

pub struct InterfaceField {
    pub doc: Option<String>,
    pub name: String,
    pub optional: bool,
    pub ty: String,
    pub mark: Option<MarkRef>,
}

/// An expression in a generated file.
pub enum Expr {
    /// Verbatim text (identifiers, type-applied callees, literals).
    Raw(String),
    /// A string literal, escaped by the printer.
    Str(String),
    Member(Box<Expr>, String),
    Call(Box<Expr>, Vec<Expr>),
    /// Always printed multiline (one entry per line).
    Object(Vec<ObjectEntry>),
    /// Printed multiline when it has more than one element.
    Array(Vec<Expr>),
    /// `(params) => body`; the body is wrapped in parens when it is an
    /// object literal.
    Arrow {
        params: Vec<String>,
        body: Box<Expr>,
    },
    /// Records the printed range of the inner expression.
    Marked(MarkRef, Box<Expr>),
}

pub struct ObjectEntry {
    pub key: String,
    pub value: Expr,
    pub mark: Option<MarkRef>,
}

impl Expr {
    pub fn ident(name: impl Into<String>) -> Self {
        Self::Raw(name.into())
    }

    pub fn call(callee: impl Into<String>, args: Vec<Expr>) -> Self {
        Self::Call(Box::new(Self::Raw(callee.into())), args)
    }

    pub fn member_call(object: impl Into<String>, name: &str, args: Vec<Expr>) -> Self {
        Self::Call(
            Box::new(Self::Member(
                Box::new(Self::Raw(object.into())),
                name.to_owned(),
            )),
            args,
        )
    }
}
