//! Shared API contract intermediate representation.

use serde::{Deserialize, Serialize};

/// Full API contract shared by collectors, generators, and tooling.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct ApiContract {
    pub package_name: String,
    pub endpoints: Vec<Endpoint>,
    pub types: Vec<TypeDef>,
    pub errors: Vec<ErrorDef>,
}

/// A callable API endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Endpoint {
    pub id: SymbolId,
    pub rust_path: Vec<String>,
    pub rust_name: String,
    pub ts_path: Vec<String>,
    pub route: RoutePattern,
    pub method: HttpMethod,
    pub transport: Transport,
    pub request: RequestShape,
    pub response: ResponseShape,
    pub errors: Vec<ErrorRef>,
    pub source: SourceRange,
    pub allow_unused: bool,
}

/// A named data type that can cross the API boundary.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct TypeDef {
    pub id: SymbolId,
    pub rust_path: Vec<String>,
    pub rust_name: String,
    pub ts_name: String,
    pub shape: TypeShape,
    pub source: SourceRange,
}

/// A named API error type.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ErrorDef {
    pub id: SymbolId,
    pub rust_path: Vec<String>,
    pub rust_name: String,
    pub ts_name: String,
    pub variants: Vec<ErrorVariant>,
    pub source: SourceRange,
}

/// Source location for diagnostics and cross-language links.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct SourceRange {
    pub file: String,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

/// Stable identity for any Rust API symbol represented in the IR.
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

impl From<String> for SymbolId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for SymbolId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// Endpoint transport style.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum Transport {
    UnaryHttp,
    ServerSentEvents,
    WebSocketDuplex,
    BinaryDownload,
    BinaryUpload,
}

/// Supported HTTP methods for unary HTTP endpoints.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum HttpMethod {
    Delete,
    Get,
    Patch,
    Post,
    Put,
}

impl HttpMethod {
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

/// Route pattern as exposed by the Rust service.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct RoutePattern(pub String);

/// Endpoint request body, path, and query shape.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct RequestShape {
    pub path_params: Vec<Field>,
    pub query_params: Vec<Field>,
    pub body: Option<TypeRef>,
}

/// Endpoint response shape.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum ResponseShape {
    Empty,
    Json(TypeRef),
    Stream(TypeRef),
}

/// Reference to a type definition.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct TypeRef {
    pub id: SymbolId,
    pub name: String,
}

/// Reference to an error definition.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ErrorRef {
    pub id: SymbolId,
    pub name: String,
}

/// Shape of a named data type.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum TypeShape {
    Primitive(Primitive),
    Struct(StructShape),
    Enum(EnumShape),
    Newtype(Box<TypeRef>),
    Tuple(Vec<TypeRef>),
    List(Box<TypeRef>),
    Map {
        key: Box<TypeRef>,
        value: Box<TypeRef>,
    },
    Option(Box<TypeRef>),
    External(ExternalType),
}

/// Primitive scalar shapes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum Primitive {
    Bool,
    I32,
    I64,
    F64,
    String,
}

/// Struct fields.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct StructShape {
    pub fields: Vec<Field>,
}

/// Enum variants.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct EnumShape {
    pub variants: Vec<EnumVariant>,
}

/// A field on a struct, enum variant, request, or error.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Field {
    pub id: SymbolId,
    pub rust_name: String,
    pub wire_name: String,
    pub ts_name: String,
    pub type_ref: TypeRef,
    pub optionality: Optionality,
    pub source: SourceRange,
}

/// Struct field presence semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum Optionality {
    Required,
    Optional,
    Nullable,
}

/// A data enum variant.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct EnumVariant {
    pub id: SymbolId,
    pub rust_name: String,
    pub wire_name: String,
    pub fields: Vec<Field>,
    pub source: SourceRange,
}

/// A type owned outside this workspace.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ExternalType {
    pub rust_path: Vec<String>,
    pub ts_import: String,
    pub ts_name: String,
}

/// A tagged error variant.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ErrorVariant {
    pub id: SymbolId,
    pub rust_name: String,
    pub tag: String,
    pub status: HttpStatus,
    pub fields: Vec<Field>,
    pub source: SourceRange,
}

/// HTTP status associated with an API error variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct HttpStatus(pub u16);

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
    fn contract_round_trips_through_json() {
        let user_id = SymbolId::from_parts("type", &["example", "User"]);
        let error_id = SymbolId::from_parts("error", &["example", "GetUserError"]);

        let contract = ApiContract {
            package_name: "example-api".to_owned(),
            endpoints: vec![
                endpoint(
                    "get_user",
                    "/users/:id",
                    HttpMethod::Get,
                    Transport::UnaryHttp,
                    ResponseShape::Json(type_ref(user_id.clone(), "User")),
                    error_id.clone(),
                ),
                endpoint(
                    "watch_users",
                    "/users/events",
                    HttpMethod::Get,
                    Transport::ServerSentEvents,
                    ResponseShape::Stream(type_ref(user_id.clone(), "User")),
                    error_id.clone(),
                ),
            ],
            types: vec![TypeDef {
                id: user_id,
                rust_path: vec!["example".to_owned(), "User".to_owned()],
                rust_name: "User".to_owned(),
                ts_name: "User".to_owned(),
                shape: TypeShape::Struct(StructShape {
                    fields: vec![Field {
                        id: SymbolId::from_parts("field", &["example", "User", "name"]),
                        rust_name: "name".to_owned(),
                        wire_name: "name".to_owned(),
                        ts_name: "name".to_owned(),
                        type_ref: type_ref(
                            SymbolId::from_parts("primitive", &["String"]),
                            "String",
                        ),
                        optionality: Optionality::Required,
                        source: source(),
                    }],
                }),
                source: source(),
            }],
            errors: vec![ErrorDef {
                id: error_id.clone(),
                rust_path: vec!["example".to_owned(), "GetUserError".to_owned()],
                rust_name: "GetUserError".to_owned(),
                ts_name: "GetUserError".to_owned(),
                variants: vec![ErrorVariant {
                    id: SymbolId::from_parts(
                        "error_variant",
                        &["example", "GetUserError", "NotFound"],
                    ),
                    rust_name: "NotFound".to_owned(),
                    tag: "not_found".to_owned(),
                    status: HttpStatus(404),
                    fields: Vec::new(),
                    source: source(),
                }],
                source: source(),
            }],
        };

        let json = serde_json::to_string_pretty(&contract).expect("serialize contract");
        let round_tripped: ApiContract = serde_json::from_str(&json).expect("deserialize contract");

        assert_eq!(round_tripped, contract);
    }

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
    fn unary_and_sse_transports_are_representable() {
        let transports = [Transport::UnaryHttp, Transport::ServerSentEvents];

        let json = serde_json::to_string(&transports).expect("serialize transports");

        assert!(json.contains("UnaryHttp"));
        assert!(json.contains("ServerSentEvents"));
    }

    fn endpoint(
        name: &str,
        route: &str,
        method: HttpMethod,
        transport: Transport,
        response: ResponseShape,
        error_id: SymbolId,
    ) -> Endpoint {
        Endpoint {
            id: SymbolId::from_parts("endpoint", &["example", name]),
            rust_path: vec!["example".to_owned(), name.to_owned()],
            rust_name: name.to_owned(),
            ts_path: vec!["client".to_owned(), name.to_owned()],
            route: RoutePattern(route.to_owned()),
            method,
            transport,
            request: RequestShape::default(),
            response,
            errors: vec![ErrorRef {
                id: error_id,
                name: "GetUserError".to_owned(),
            }],
            source: source(),
            allow_unused: false,
        }
    }

    fn source() -> SourceRange {
        SourceRange {
            file: "src/lib.rs".to_owned(),
            start_line: 1,
            start_column: 1,
            end_line: 1,
            end_column: 10,
        }
    }

    fn type_ref(id: SymbolId, name: &str) -> TypeRef {
        TypeRef {
            id,
            name: name.to_owned(),
        }
    }
}
