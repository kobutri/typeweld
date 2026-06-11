//! The typeweld contract intermediate representation.
//!
//! A [`Contract`] is the complete, serializable description of one generated
//! API package: its endpoints, the named types reachable from them, and the
//! API error declarations. It is produced by the static extractor and consumed
//! by the TypeScript generator, the CLI lints, and the language server.

use serde::{Deserialize, Serialize};

/// Bumped whenever the serialized contract shape changes.
pub const CONTRACT_SCHEMA_VERSION: u32 = 3;

/// The full API contract for one generated TypeScript package.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Contract {
    pub schema_version: u32,
    pub package: PackageMeta,
    pub endpoints: Vec<Endpoint>,
    pub types: Vec<TypeDecl>,
    pub errors: Vec<ErrorDecl>,
}

/// Identity of the generated package and its source crate.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct PackageMeta {
    /// The generated TypeScript package name, e.g. `@workspace/server-api`.
    pub ts_package: String,
    /// The cargo package the contract was extracted from.
    pub cargo_package: String,
}

/// A callable API endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Endpoint {
    pub id: SymbolId,
    /// Fully qualified Rust path including the function name.
    pub rust_path: Vec<String>,
    pub rust_name: String,
    /// Generated TypeScript module under `endpoints/`, e.g. `users`.
    pub ts_module: String,
    /// Generated TypeScript accessor name, e.g. `getUser`.
    pub ts_name: String,
    /// Effective route after router nesting, e.g. `/api/users/{id}`.
    pub route: String,
    pub method: HttpMethod,
    pub request: RequestShape,
    pub success: SuccessChannel,
    pub errors: Vec<ErrorRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    pub spans: DeclSpans,
    /// Locations where the endpoint is mounted in `#[api_router]` functions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub router_mounts: Vec<Span>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_unused: bool,
}

/// Path params, query params, and request body of an endpoint.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct RequestShape {
    pub path_params: Vec<Param>,
    pub query_params: Vec<Param>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<RequestBody>,
}

/// One path or query parameter.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Param {
    pub id: SymbolId,
    /// The wire name (path segment name or query key).
    pub name: String,
    pub ty: TypeExpr,
    /// Query params from optional fields may be omitted; path params never.
    pub required: bool,
    pub span: Span,
}

/// The request body channel.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum RequestBody {
    Json(TypeExpr),
    Binary,
}

/// The success channel carried by a generated Effect or Stream accessor.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct SuccessChannel {
    pub transport: SuccessTransport,
    pub status: u16,
    pub body: SuccessBody,
}

/// Endpoint transport style.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum SuccessTransport {
    Unary,
    Sse,
    BinaryDownload,
    BinaryUpload,
}

/// Success body shape.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum SuccessBody {
    Empty,
    Json(TypeExpr),
    Stream(TypeExpr),
    Binary,
}

/// A structural type expression appearing in fields, params, and bodies.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum TypeExpr {
    Primitive(Primitive),
    /// Reference to a named [`TypeDecl`] in the same contract.
    Named(TypeRef),
    Option(Box<TypeExpr>),
    List(Box<TypeExpr>),
    Map {
        key: Box<TypeExpr>,
        value: Box<TypeExpr>,
    },
    External(ExternalType),
}

/// Reference to a named type declaration.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct TypeRef {
    pub id: SymbolId,
    /// The generated TypeScript name, kept for readable contracts.
    pub name: String,
}

/// Reference to an error declaration.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ErrorRef {
    pub id: SymbolId,
    pub name: String,
}

/// Primitive scalar shapes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum Primitive {
    Bool,
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    I128,
    U128,
    Usize,
    Isize,
    F32,
    F64,
    String,
}

impl Primitive {
    /// Parses a Rust primitive type name.
    #[must_use]
    pub fn from_rust_name(name: &str) -> Option<Self> {
        Some(match name {
            "bool" => Self::Bool,
            "i8" => Self::I8,
            "u8" => Self::U8,
            "i16" => Self::I16,
            "u16" => Self::U16,
            "i32" => Self::I32,
            "u32" => Self::U32,
            "i64" => Self::I64,
            "u64" => Self::U64,
            "i128" => Self::I128,
            "u128" => Self::U128,
            "usize" => Self::Usize,
            "isize" => Self::Isize,
            "f32" => Self::F32,
            "f64" => Self::F64,
            "String" | "str" => Self::String,
            _ => return None,
        })
    }

    /// Whether the primitive is encoded as a JSON string on the wire because
    /// it exceeds the safe integer range of TypeScript `number`.
    #[must_use]
    pub const fn is_big_int(self) -> bool {
        matches!(
            self,
            Self::I64 | Self::U64 | Self::I128 | Self::U128 | Self::Usize | Self::Isize
        )
    }
}

/// Well-known types owned outside the workspace, with fixed wire mappings.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum ExternalType {
    /// `uuid::Uuid`, a string on the wire.
    Uuid,
    /// `chrono::DateTime<chrono::Utc>`, an RFC 3339 string on the wire.
    DateTimeUtc,
    /// `rust_decimal::Decimal`, a string on the wire.
    Decimal,
    /// `serde_json::Value`, opaque JSON.
    JsonValue,
}

/// A named data type crossing the API boundary.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct TypeDecl {
    pub id: SymbolId,
    pub rust_path: Vec<String>,
    pub rust_name: String,
    pub ts_name: String,
    pub shape: TypeShape,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    pub spans: DeclSpans,
}

/// Shape of a named type declaration.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum TypeShape {
    Struct { fields: Vec<Field> },
    Enum { variants: Vec<EnumVariant> },
    Newtype(TypeExpr),
}

/// A named field of a struct, enum variant, or error variant.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Field {
    pub id: SymbolId,
    pub rust_name: String,
    /// The serde wire name, which is also the generated TypeScript name.
    pub wire_name: String,
    pub ty: TypeExpr,
    pub optionality: Optionality,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    pub spans: DeclSpans,
}

/// Wire-level field presence semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum Optionality {
    /// Always present.
    Required,
    /// May be absent (serde default).
    Optional,
    /// Always present, may be null (`Option<T>`).
    Nullable,
    /// May be absent or null.
    OptionalNullable,
}

/// A data enum variant (internally tagged with `_tag`).
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct EnumVariant {
    pub id: SymbolId,
    pub rust_name: String,
    /// The `_tag` discriminator value on the wire.
    pub wire_name: String,
    pub fields: Vec<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    pub spans: DeclSpans,
}

/// A named API error enum.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ErrorDecl {
    pub id: SymbolId,
    pub rust_path: Vec<String>,
    pub rust_name: String,
    pub ts_name: String,
    pub variants: Vec<ErrorVariant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    pub spans: DeclSpans,
}

/// A tagged error variant with its HTTP status.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ErrorVariant {
    pub id: SymbolId,
    pub rust_name: String,
    /// The `_tag` discriminator value on the wire.
    pub tag: String,
    pub status: u16,
    pub fields: Vec<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    pub spans: DeclSpans,
}

/// Supported HTTP methods.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum HttpMethod {
    Delete,
    Get,
    Patch,
    Post,
    Put,
}

impl HttpMethod {
    /// The method name as written on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Delete => "DELETE",
            Self::Get => "GET",
            Self::Patch => "PATCH",
            Self::Post => "POST",
            Self::Put => "PUT",
        }
    }
}

/// A byte-offset span in a workspace source file.
///
/// Line/column positions are derived on demand from file contents via
/// [`crate::line_index::LineIndex`]; the contract stores only byte offsets so
/// there is exactly one source of truth for position encoding.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct Span {
    /// Workspace-root-relative path with `/` separators.
    pub file: String,
    pub start: u32,
    pub end: u32,
}

impl Span {
    /// Whether the span contains the byte `offset` in `file`.
    #[must_use]
    pub fn contains(&self, file: &str, offset: u32) -> bool {
        self.file == file && offset >= self.start && offset <= self.end
    }
}

/// The spans of one declaration.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct DeclSpans {
    /// The identifier being declared (rename target).
    pub name: Span,
    /// The whole item.
    pub full: Span,
    /// The `#[api]` / `#[status]` attribute, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attr: Option<Span>,
}

/// Stable identity for any Rust API symbol represented in the contract.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
pub struct SymbolId(String);

impl SymbolId {
    /// Creates a symbol id from an already persisted stable value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Builds a deterministic symbol id from a namespace and symbol path.
    #[must_use]
    pub fn from_parts(namespace: &str, parts: &[&str]) -> Self {
        let mut hash = FNV_OFFSET_BASIS;

        hash = fnv1a(hash, namespace.as_bytes());
        for part in parts {
            hash = fnv1a(hash, &[0]);
            hash = fnv1a(hash, part.as_bytes());
        }

        Self(format!("{namespace}:{hash:016x}"))
    }

    /// Returns the serialized symbol id value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_ids_are_deterministic() {
        let first = SymbolId::from_parts("endpoint", &["example", "get_user"]);
        let second = SymbolId::from_parts("endpoint", &["example", "get_user"]);
        let different = SymbolId::from_parts("endpoint", &["example", "delete_user"]);

        assert_eq!(first, second);
        assert_ne!(first, different);
        assert_eq!(first.as_str(), "endpoint:dd7ba083e0f14f7c");
    }

    #[test]
    fn contract_round_trips_through_json() {
        let user = TypeRef {
            id: SymbolId::from_parts("type", &["example", "User"]),
            name: "User".to_owned(),
        };
        let contract = Contract {
            schema_version: CONTRACT_SCHEMA_VERSION,
            package: PackageMeta {
                ts_package: "@workspace/example-api".to_owned(),
                cargo_package: "example".to_owned(),
            },
            endpoints: vec![Endpoint {
                id: SymbolId::from_parts("endpoint", &["example", "get_user"]),
                rust_path: vec!["example".to_owned(), "get_user".to_owned()],
                rust_name: "get_user".to_owned(),
                ts_module: "users".to_owned(),
                ts_name: "getUser".to_owned(),
                route: "/users/{id}".to_owned(),
                method: HttpMethod::Get,
                request: RequestShape {
                    path_params: vec![Param {
                        id: SymbolId::from_parts("param", &["example", "get_user", "id"]),
                        name: "id".to_owned(),
                        ty: TypeExpr::Primitive(Primitive::I32),
                        required: true,
                        span: Span::default(),
                    }],
                    query_params: Vec::new(),
                    body: None,
                },
                success: SuccessChannel {
                    transport: SuccessTransport::Unary,
                    status: 200,
                    body: SuccessBody::Json(TypeExpr::Named(user.clone())),
                },
                errors: Vec::new(),
                docs: Some("Fetch one user.".to_owned()),
                spans: DeclSpans::default(),
                router_mounts: Vec::new(),
                allow_unused: false,
            }],
            types: vec![TypeDecl {
                id: user.id,
                rust_path: vec!["example".to_owned(), "User".to_owned()],
                rust_name: "User".to_owned(),
                ts_name: "User".to_owned(),
                shape: TypeShape::Struct {
                    fields: vec![Field {
                        id: SymbolId::from_parts("field", &["example::User", "name"]),
                        rust_name: "name".to_owned(),
                        wire_name: "name".to_owned(),
                        ty: TypeExpr::Option(Box::new(TypeExpr::Primitive(Primitive::String))),
                        optionality: Optionality::Nullable,
                        docs: None,
                        spans: DeclSpans::default(),
                    }],
                },
                docs: None,
                spans: DeclSpans::default(),
            }],
            errors: Vec::new(),
        };

        let json = serde_json::to_string_pretty(&contract).expect("serialize");
        let round_tripped: Contract = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(round_tripped, contract);
    }

    #[test]
    fn big_int_primitives_are_flagged() {
        assert!(Primitive::I64.is_big_int());
        assert!(Primitive::U128.is_big_int());
        assert!(!Primitive::I32.is_big_int());
        assert!(!Primitive::F64.is_big_int());
    }
}
