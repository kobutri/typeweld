//! Framework-neutral public API primitives.

pub use api_ir as ir;
use api_ir::{
    ErrorDef, ErrorRef, Field, HttpMethod, Optionality, Primitive, RequestShape, ResponseShape,
    RoutePattern, SourceRange, StructShape, SymbolId, Transport, TypeDef, TypeRef, TypeShape,
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
            shape: TypeShape::External(api_ir::ExternalType {
                rust_path: Self::rust_path(),
                ts_import: String::new(),
                ts_name: Self::TS_NAME.to_owned(),
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
}
