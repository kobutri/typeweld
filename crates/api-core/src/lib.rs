//! Framework-neutral public API primitives.

use std::collections::{BTreeMap, HashMap};

pub use api_ir as ir;
use api_ir::{
    ErrorDef, ErrorRef, ExternalType, Field, HttpMethod, Optionality, Primitive, RequestShape,
    ResponseShape, RoutePattern, SourceRange, StructShape, SymbolId, Transport, TypeDef, TypeRef,
    TypeShape,
};

/// Rust type that can cross the generated API boundary.
///
/// Implementations are pure metadata providers. They do not depend on any web
/// framework, serializer, or runtime transport.
pub trait ApiType {
    /// Stable Rust-facing type name.
    const RUST_NAME: &'static str;

    /// Stable TypeScript-facing type name.
    const TS_NAME: &'static str = Self::RUST_NAME;

    /// Fully-qualified Rust path, represented as path segments.
    fn rust_path() -> Vec<String> {
        vec![Self::RUST_NAME.to_owned()]
    }

    /// Reference used by endpoint and field metadata.
    fn type_ref() -> TypeRef {
        TypeRef {
            id: SymbolId::from_parts("type", &[Self::RUST_NAME]),
            name: Self::TS_NAME.to_owned(),
        }
    }

    /// Full type definition when this type is exported in a contract.
    fn type_def() -> TypeDef {
        TypeDef {
            id: Self::type_ref().id,
            rust_path: Self::rust_path(),
            rust_name: Self::RUST_NAME.to_owned(),
            ts_name: Self::TS_NAME.to_owned(),
            shape: TypeShape::External(ExternalType {
                rust_path: Self::rust_path(),
                ts_import: String::new(),
                ts_name: Self::TS_NAME.to_owned(),
                encoded_ts_name: Self::TS_NAME.to_owned(),
                decoded_ts_name: Self::TS_NAME.to_owned(),
            }),
            source: SourceRange::default(),
        }
    }
}

/// Declared, typed API error.
pub trait ApiError: ApiType {
    /// Reference used by endpoint metadata.
    fn error_ref() -> ErrorRef {
        ErrorRef {
            id: SymbolId::from_parts("error", &[Self::RUST_NAME]),
            name: Self::TS_NAME.to_owned(),
        }
    }

    /// HTTP status for this concrete domain error value.
    fn status(&self) -> api_ir::HttpStatus;

    /// Full error definition when this error is exported in a contract.
    fn error_def() -> ErrorDef;
}

/// Framework-neutral endpoint descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
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

impl Endpoint {
    #[must_use]
    pub fn new(method: HttpMethod, path: impl Into<String>) -> Self {
        let path = path.into();
        Self {
            id: SymbolId::from_parts("endpoint", &[method.as_str(), &path]),
            rust_path: Vec::new(),
            rust_name: String::new(),
            ts_path: Vec::new(),
            route: RoutePattern(path),
            method,
            transport: Transport::UnaryHttp,
            request: RequestShape::default(),
            response: ResponseShape::Empty,
            errors: Vec::new(),
            source: SourceRange::default(),
            allow_unused: false,
        }
    }

    #[must_use]
    pub fn named(mut self, rust_path: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.rust_path = rust_path.into_iter().map(Into::into).collect();
        self.rust_name = self.rust_path.last().cloned().unwrap_or_default();
        if self.ts_path.is_empty() {
            self.ts_path = vec![self.rust_name.clone()];
        }
        self.id = SymbolId::from_parts(
            "endpoint",
            &self
                .rust_path
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        self
    }

    #[must_use]
    pub fn request(mut self, request: RequestShape) -> Self {
        self.request = request;
        self
    }

    #[must_use]
    pub fn response(mut self, response: ResponseShape) -> Self {
        self.response = response;
        self
    }

    #[must_use]
    pub fn errors(mut self, errors: impl IntoIterator<Item = ErrorRef>) -> Self {
        self.errors = errors.into_iter().collect();
        self
    }

    #[must_use]
    pub const fn allow_unused(mut self, allow_unused: bool) -> Self {
        self.allow_unused = allow_unused;
        self
    }

    #[must_use]
    pub fn into_ir(self) -> api_ir::Endpoint {
        api_ir::Endpoint {
            id: self.id,
            rust_path: self.rust_path,
            rust_name: self.rust_name,
            ts_path: self.ts_path,
            route: self.route,
            method: self.method,
            transport: self.transport,
            request: self.request,
            response: self.response,
            errors: self.errors,
            source: self.source,
            allow_unused: self.allow_unused,
        }
    }
}

/// JSON response wrapper.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Json<T>(pub T);

/// `201 Created` JSON response wrapper.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Created<T>(pub T);

/// Empty `204 No Content` response marker.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NoContent;

/// Path extractor marker.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Path<T>(pub T);

/// Query extractor marker.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Query<T>(pub T);

/// Body extractor marker.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Body<T>(pub T);

/// Explicit root module for exported API endpoints.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ApiModule {
    pub name: String,
    pub endpoints: Vec<Endpoint>,
}

/// Reachability summary for exported endpoint collection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EndpointReachability {
    pub root_module: String,
    pub endpoint_ids: Vec<SymbolId>,
}

impl ApiModule {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            endpoints: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_endpoint(mut self, endpoint: Endpoint) -> Self {
        self.endpoints.push(endpoint);
        self
    }

    #[must_use]
    pub fn endpoint_irs(&self) -> Vec<api_ir::Endpoint> {
        self.endpoints
            .iter()
            .cloned()
            .map(Endpoint::into_ir)
            .collect()
    }

    #[must_use]
    pub fn reachability_graph(&self) -> EndpointReachability {
        EndpointReachability {
            root_module: self.name.clone(),
            endpoint_ids: self
                .endpoints
                .iter()
                .map(|endpoint| endpoint.id.clone())
                .collect(),
        }
    }
}

/// Compose the explicit root API module exported by a crate.
#[macro_export]
macro_rules! api_module {
    (name = $name:literal, endpoints = [$($endpoint:path),* $(,)?]) => {{
        let mut module = $crate::ApiModule::new($name);
        $(
            module = module.with_endpoint($endpoint());
        )*
        module
    }};
}

impl<T: ApiType> ApiType for Json<T> {
    const RUST_NAME: &'static str = T::RUST_NAME;
    const TS_NAME: &'static str = T::TS_NAME;

    fn rust_path() -> Vec<String> {
        T::rust_path()
    }

    fn type_ref() -> TypeRef {
        T::type_ref()
    }

    fn type_def() -> TypeDef {
        T::type_def()
    }
}

impl<T: ApiType> ApiType for Created<T> {
    const RUST_NAME: &'static str = T::RUST_NAME;
    const TS_NAME: &'static str = T::TS_NAME;

    fn rust_path() -> Vec<String> {
        T::rust_path()
    }

    fn type_ref() -> TypeRef {
        T::type_ref()
    }

    fn type_def() -> TypeDef {
        T::type_def()
    }
}

impl ApiType for NoContent {
    const RUST_NAME: &'static str = "NoContent";

    fn type_def() -> TypeDef {
        TypeDef {
            id: Self::type_ref().id,
            rust_path: Self::rust_path(),
            rust_name: Self::RUST_NAME.to_owned(),
            ts_name: Self::TS_NAME.to_owned(),
            shape: TypeShape::Struct(StructShape::default()),
            source: SourceRange::default(),
        }
    }
}

impl<T: ApiType> ApiType for Path<T> {
    const RUST_NAME: &'static str = T::RUST_NAME;
    const TS_NAME: &'static str = T::TS_NAME;

    fn rust_path() -> Vec<String> {
        T::rust_path()
    }

    fn type_ref() -> TypeRef {
        T::type_ref()
    }

    fn type_def() -> TypeDef {
        T::type_def()
    }
}

impl<T: ApiType> ApiType for Query<T> {
    const RUST_NAME: &'static str = T::RUST_NAME;
    const TS_NAME: &'static str = T::TS_NAME;

    fn rust_path() -> Vec<String> {
        T::rust_path()
    }

    fn type_ref() -> TypeRef {
        T::type_ref()
    }

    fn type_def() -> TypeDef {
        T::type_def()
    }
}

impl<T: ApiType> ApiType for Body<T> {
    const RUST_NAME: &'static str = T::RUST_NAME;
    const TS_NAME: &'static str = T::TS_NAME;

    fn rust_path() -> Vec<String> {
        T::rust_path()
    }

    fn type_ref() -> TypeRef {
        T::type_ref()
    }

    fn type_def() -> TypeDef {
        T::type_def()
    }
}

macro_rules! primitive_api_type {
    ($ty:ty, $rust_name:literal, $primitive:expr) => {
        impl ApiType for $ty {
            const RUST_NAME: &'static str = $rust_name;

            fn type_ref() -> TypeRef {
                TypeRef {
                    id: SymbolId::from_parts("primitive", &[$rust_name]),
                    name: $rust_name.to_owned(),
                }
            }

            fn type_def() -> TypeDef {
                TypeDef {
                    id: Self::type_ref().id,
                    rust_path: vec![$rust_name.to_owned()],
                    rust_name: $rust_name.to_owned(),
                    ts_name: $rust_name.to_owned(),
                    shape: TypeShape::Primitive($primitive),
                    source: SourceRange::default(),
                }
            }
        }
    };
}

primitive_api_type!(bool, "bool", Primitive::Bool);
primitive_api_type!(i32, "i32", Primitive::I32);
primitive_api_type!(i64, "i64", Primitive::I64);
primitive_api_type!(f64, "f64", Primitive::F64);
primitive_api_type!(String, "String", Primitive::String);

/// Integer encoding policy for generated clients.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegerEncodingPolicy {
    SafeNumber,
    StringEncoded,
}

#[must_use]
pub const fn integer_policy_for(rust_name: &str) -> IntegerEncodingPolicy {
    match rust_name.as_bytes() {
        b"i64" | b"u64" | b"usize" | b"isize" => IntegerEncodingPolicy::StringEncoded,
        _ => IntegerEncodingPolicy::SafeNumber,
    }
}

impl ApiType for &str {
    const RUST_NAME: &'static str = "str";
    const TS_NAME: &'static str = "String";

    fn type_ref() -> TypeRef {
        <String as ApiType>::type_ref()
    }

    fn type_def() -> TypeDef {
        <String as ApiType>::type_def()
    }
}

impl<T: ApiType> ApiType for Option<T> {
    const RUST_NAME: &'static str = T::RUST_NAME;
    const TS_NAME: &'static str = T::TS_NAME;

    fn rust_path() -> Vec<String> {
        T::rust_path()
    }

    fn type_ref() -> TypeRef {
        TypeRef {
            id: SymbolId::from_parts("option", &[T::RUST_NAME]),
            name: format!("{}?", T::TS_NAME),
        }
    }

    fn type_def() -> TypeDef {
        TypeDef {
            id: Self::type_ref().id,
            rust_path: T::rust_path(),
            rust_name: T::RUST_NAME.to_owned(),
            ts_name: T::TS_NAME.to_owned(),
            shape: TypeShape::Option(Box::new(T::type_ref())),
            source: SourceRange::default(),
        }
    }
}

impl<T: ApiType> ApiType for Vec<T> {
    const RUST_NAME: &'static str = "Vec";
    const TS_NAME: &'static str = "Array";

    fn type_ref() -> TypeRef {
        TypeRef {
            id: SymbolId::from_parts("list", &[T::RUST_NAME]),
            name: format!("Array<{}>", T::TS_NAME),
        }
    }

    fn type_def() -> TypeDef {
        TypeDef {
            id: Self::type_ref().id,
            rust_path: vec!["Vec".to_owned(), T::RUST_NAME.to_owned()],
            rust_name: "Vec".to_owned(),
            ts_name: Self::type_ref().name,
            shape: TypeShape::List(Box::new(T::type_ref())),
            source: SourceRange::default(),
        }
    }
}

impl<T: ApiType> ApiType for BTreeMap<String, T> {
    const RUST_NAME: &'static str = "BTreeMap";
    const TS_NAME: &'static str = "Record";

    fn type_ref() -> TypeRef {
        TypeRef {
            id: SymbolId::from_parts("map", &["String", T::RUST_NAME]),
            name: format!("Record<string, {}>", T::TS_NAME),
        }
    }

    fn type_def() -> TypeDef {
        TypeDef {
            id: Self::type_ref().id,
            rust_path: vec![
                "BTreeMap".to_owned(),
                "String".to_owned(),
                T::RUST_NAME.to_owned(),
            ],
            rust_name: "BTreeMap".to_owned(),
            ts_name: Self::type_ref().name,
            shape: TypeShape::Map {
                key: Box::new(String::type_ref()),
                value: Box::new(T::type_ref()),
            },
            source: SourceRange::default(),
        }
    }
}

impl<T: ApiType> ApiType for HashMap<String, T> {
    const RUST_NAME: &'static str = "HashMap";
    const TS_NAME: &'static str = "Record";

    fn type_ref() -> TypeRef {
        TypeRef {
            id: SymbolId::from_parts("map", &["String", T::RUST_NAME]),
            name: format!("Record<string, {}>", T::TS_NAME),
        }
    }

    fn type_def() -> TypeDef {
        TypeDef {
            id: Self::type_ref().id,
            rust_path: vec![
                "HashMap".to_owned(),
                "String".to_owned(),
                T::RUST_NAME.to_owned(),
            ],
            rust_name: "HashMap".to_owned(),
            ts_name: Self::type_ref().name,
            shape: TypeShape::Map {
                key: Box::new(String::type_ref()),
                value: Box::new(T::type_ref()),
            },
            source: SourceRange::default(),
        }
    }
}

impl ApiType for uuid::Uuid {
    const RUST_NAME: &'static str = "Uuid";

    fn rust_path() -> Vec<String> {
        vec!["uuid".to_owned(), "Uuid".to_owned()]
    }

    fn type_ref() -> TypeRef {
        TypeRef {
            id: SymbolId::from_parts("external", &["uuid", "Uuid"]),
            name: "Uuid".to_owned(),
        }
    }

    fn type_def() -> TypeDef {
        external_type_def::<Self>("@api/external", "Uuid", "string", "Uuid")
    }
}

impl ApiType for chrono::DateTime<chrono::Utc> {
    const RUST_NAME: &'static str = "DateTimeUtc";
    const TS_NAME: &'static str = "Date";

    fn rust_path() -> Vec<String> {
        vec!["chrono".to_owned(), "DateTime".to_owned(), "Utc".to_owned()]
    }

    fn type_ref() -> TypeRef {
        TypeRef {
            id: SymbolId::from_parts("external", &["chrono", "DateTime", "Utc"]),
            name: "Date".to_owned(),
        }
    }

    fn type_def() -> TypeDef {
        external_type_def::<Self>("@api/external", "Date", "string", "Date")
    }
}

impl ApiType for rust_decimal::Decimal {
    const RUST_NAME: &'static str = "Decimal";

    fn rust_path() -> Vec<String> {
        vec!["rust_decimal".to_owned(), "Decimal".to_owned()]
    }

    fn type_ref() -> TypeRef {
        TypeRef {
            id: SymbolId::from_parts("external", &["rust_decimal", "Decimal"]),
            name: "Decimal".to_owned(),
        }
    }

    fn type_def() -> TypeDef {
        external_type_def::<Self>("@api/external", "Decimal", "string", "Decimal")
    }
}

impl ApiType for serde_json::Value {
    const RUST_NAME: &'static str = "JsonValue";
    const TS_NAME: &'static str = "JsonValue";

    fn rust_path() -> Vec<String> {
        vec!["serde_json".to_owned(), "Value".to_owned()]
    }

    fn type_ref() -> TypeRef {
        TypeRef {
            id: SymbolId::from_parts("external", &["serde_json", "Value"]),
            name: "JsonValue".to_owned(),
        }
    }

    fn type_def() -> TypeDef {
        external_type_def::<Self>("@api/external", "JsonValue", "unknown", "JsonValue")
    }
}

fn external_type_def<T: ApiType>(
    ts_import: &str,
    ts_name: &str,
    encoded_ts_name: &str,
    decoded_ts_name: &str,
) -> TypeDef {
    TypeDef {
        id: T::type_ref().id,
        rust_path: T::rust_path(),
        rust_name: T::RUST_NAME.to_owned(),
        ts_name: ts_name.to_owned(),
        shape: TypeShape::External(ExternalType {
            rust_path: T::rust_path(),
            ts_import: ts_import.to_owned(),
            ts_name: ts_name.to_owned(),
            encoded_ts_name: encoded_ts_name.to_owned(),
            decoded_ts_name: decoded_ts_name.to_owned(),
        }),
        source: SourceRange::default(),
    }
}

/// Build a field descriptor for manual trait implementations and macro output.
#[must_use]
pub fn field<T: ApiType>(owner: &str, rust_name: &str) -> Field {
    Field {
        id: SymbolId::from_parts("field", &[owner, rust_name]),
        rust_name: rust_name.to_owned(),
        wire_name: rust_name.to_owned(),
        ts_name: rust_name.to_owned(),
        type_ref: T::type_ref(),
        optionality: Optionality::Required,
        source: SourceRange::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    struct User {
        id: i64,
    }

    impl ApiType for User {
        const RUST_NAME: &'static str = "User";

        fn type_def() -> TypeDef {
            TypeDef {
                id: Self::type_ref().id,
                rust_path: Self::rust_path(),
                rust_name: Self::RUST_NAME.to_owned(),
                ts_name: Self::TS_NAME.to_owned(),
                shape: TypeShape::Struct(StructShape {
                    fields: vec![field::<i64>(Self::RUST_NAME, "id")],
                }),
                source: SourceRange::default(),
            }
        }
    }

    #[test]
    fn manual_trait_impl_can_emit_metadata() {
        let type_def = User::type_def();

        assert_eq!(type_def.rust_name, "User");
        assert!(matches!(type_def.shape, TypeShape::Struct(_)));
    }

    #[test]
    fn wrappers_are_framework_neutral() {
        let response = ResponseShape::Json(<Json<User> as ApiType>::type_ref());
        let endpoint = Endpoint::new(HttpMethod::Get, "/users/{id}")
            .named(["crate", "get_user"])
            .response(response)
            .allow_unused(true);

        assert_eq!(endpoint.method.as_str(), "GET");
        assert_eq!(endpoint.route.0, "/users/{id}");
        assert!(endpoint.allow_unused);
    }

    #[test]
    fn api_module_exports_endpoint_ir() {
        let module =
            ApiModule::new("users").with_endpoint(Endpoint::new(HttpMethod::Post, "/users"));

        assert_eq!(module.endpoint_irs().len(), 1);
    }

    #[test]
    fn api_module_macro_exports_only_listed_endpoints() {
        fn get_user() -> Endpoint {
            Endpoint::new(HttpMethod::Get, "/users/{id}").named(["crate", "get_user"])
        }

        fn delete_user() -> Endpoint {
            Endpoint::new(HttpMethod::Delete, "/users/{id}").named(["crate", "delete_user"])
        }

        let module = api_module!(name = "users", endpoints = [get_user]);
        let graph = module.reachability_graph();

        assert_eq!(module.name, "users");
        assert_eq!(module.endpoints.len(), 1);
        assert_eq!(module.endpoints[0].rust_name, "get_user");
        assert_eq!(graph.endpoint_ids, vec![get_user().id]);
        assert_ne!(module.endpoints[0].id, delete_user().id);
    }

    #[test]
    fn common_external_mappings_preserve_encoded_and_decoded_names() {
        let TypeShape::External(uuid) = uuid::Uuid::type_def().shape else {
            panic!("expected external uuid mapping");
        };
        let TypeShape::External(date) = chrono::DateTime::<chrono::Utc>::type_def().shape else {
            panic!("expected external date mapping");
        };
        let TypeShape::External(decimal) = rust_decimal::Decimal::type_def().shape else {
            panic!("expected external decimal mapping");
        };

        assert_eq!(uuid.encoded_ts_name, "string");
        assert_eq!(uuid.decoded_ts_name, "Uuid");
        assert_eq!(date.encoded_ts_name, "string");
        assert_eq!(date.decoded_ts_name, "Date");
        assert_eq!(decimal.encoded_ts_name, "string");
        assert_eq!(decimal.decoded_ts_name, "Decimal");
    }

    #[test]
    fn list_map_option_and_integer_policy_are_represented() {
        assert!(matches!(
            Vec::<String>::type_def().shape,
            TypeShape::List(_)
        ));
        assert!(matches!(
            BTreeMap::<String, i64>::type_def().shape,
            TypeShape::Map { .. }
        ));
        assert!(matches!(
            Option::<String>::type_def().shape,
            TypeShape::Option(_)
        ));
        assert_eq!(
            integer_policy_for("i64"),
            IntegerEncodingPolicy::StringEncoded
        );
        assert_eq!(integer_policy_for("i32"), IntegerEncodingPolicy::SafeNumber);
    }
}
