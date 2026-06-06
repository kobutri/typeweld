//! Axum integration adapter.
//!
//! Use this crate's `Json`, `Path`, `Query`, `Body`, `Binary`, `Created`,
//! `NoContent`, and `Sse` wrappers in Axum handler signatures. The `#[api]`
//! macro reads those signatures into the same framework-neutral contract shapes
//! used by `api_core`.

use std::{convert::Infallible, marker::PhantomData, ops::Deref};

use api_core::{
    ir::{HttpMethod, HttpStatus, SourceRange},
    ApiError, ApiRouteNode, ApiRouteTree, EndpointDescriptor, IntoApiRoot, MountedEndpoint,
};
pub use axum::response::Response;
use axum::{
    body::Bytes,
    extract::{FromRequest, FromRequestParts, Request},
    handler::Handler,
    http::{header::CONTENT_TYPE, request::Parts, HeaderValue, StatusCode},
    response::{
        sse::{Event, Sse as AxumSse},
        IntoResponse,
    },
    routing::{delete, get, patch, post, put, MethodRouter, Route},
    Router,
};
use futures_util::{Stream, StreamExt};
use serde::Serialize;
use tower_layer::Layer;
use tower_service::Service;

/// Axum router builder paired with an API route tree.
#[derive(Clone, Debug)]
pub struct ApiRouter<S = ()> {
    module_name: String,
    router: Router<S>,
    tree: ApiRouteTree,
}

impl ApiRouter<()> {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        let module_name = name.into();
        Self {
            tree: ApiRouteTree::new(module_name.clone()),
            module_name,
            router: Router::new(),
        }
    }
}

impl<S> ApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    #[must_use]
    pub fn with_state<S2>(self, state: S) -> ApiRouter<S2>
    where
        S2: Clone + Send + Sync + 'static,
    {
        ApiRouter {
            module_name: self.module_name,
            tree: self.tree,
            router: self.router.with_state(state),
        }
    }

    /// Register a handler using the endpoint descriptor's declared method and route.
    #[must_use]
    pub fn endpoint_descriptor<H, T>(
        mut self,
        descriptor: EndpointDescriptor,
        handler: H,
        source: SourceRange,
    ) -> Self
    where
        H: Handler<T, S>,
        T: 'static,
    {
        let endpoint = descriptor.endpoint.clone();
        let method = endpoint.method;
        let path = endpoint.route.0.clone();
        self.router = self
            .router
            .route(&normalize_route_path(&path), method_router(method, handler));
        self.tree
            .nodes
            .push(ApiRouteNode::Endpoint(MountedEndpoint {
                descriptor,
                source,
                effective_method: method,
                effective_path: path,
            }));
        self
    }

    #[must_use]
    pub fn route<R, H, T>(self, route: R, handler: H) -> Self
    where
        R: ApiRouteRegistration<H, T, S>,
    {
        route.register(self, handler)
    }

    #[must_use]
    pub fn route_service<T>(mut self, path: &str, service: T) -> Self
    where
        T: Service<Request, Error = Infallible> + Clone + Send + Sync + 'static,
        T::Response: IntoResponse,
        T::Future: Send + 'static,
    {
        self.router = self.router.route_service(path, service);
        self.tree.nodes.push(ApiRouteNode::RawService {
            path: path.to_owned(),
            source: SourceRange::default(),
        });
        self
    }

    #[must_use]
    pub fn nest(mut self, path: &str, router: ApiRouter<S>) -> Self {
        self.router = self.router.nest(path, router.router);
        self.tree.nodes.push(ApiRouteNode::Nest {
            path: path.to_owned(),
            source: SourceRange::default(),
            child: Box::new(router.tree),
        });
        self
    }

    #[must_use]
    pub fn merge(mut self, router: ApiRouter<S>) -> Self {
        self.router = self.router.merge(router.router);
        self.tree.nodes.push(ApiRouteNode::Merge {
            source: SourceRange::default(),
            child: Box::new(router.tree),
        });
        self
    }

    #[must_use]
    pub fn layer<L>(mut self, layer: L) -> Self
    where
        L: Layer<Route> + Clone + Send + Sync + 'static,
        L::Service: Service<Request> + Clone + Send + Sync + 'static,
        <L::Service as Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as Service<Request>>::Error: Into<Infallible> + 'static,
        <L::Service as Service<Request>>::Future: Send + 'static,
    {
        let child_count = self.tree.nodes.len();
        self.router = self.router.layer(layer);
        self.tree.nodes.push(ApiRouteNode::Layer {
            source: SourceRange::default(),
            child_count,
        });
        self
    }

    #[must_use]
    pub fn route_layer<L>(mut self, layer: L) -> Self
    where
        L: Layer<Route> + Clone + Send + Sync + 'static,
        L::Service: Service<Request> + Clone + Send + Sync + 'static,
        <L::Service as Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as Service<Request>>::Error: Into<Infallible> + 'static,
        <L::Service as Service<Request>>::Future: Send + 'static,
    {
        let child_count = self.tree.nodes.len();
        self.router = self.router.route_layer(layer);
        self.tree.nodes.push(ApiRouteNode::RouteLayer {
            source: SourceRange::default(),
            child_count,
        });
        self
    }

    #[must_use]
    pub fn fallback<H, T>(mut self, handler: H) -> Self
    where
        H: Handler<T, S>,
        T: 'static,
    {
        self.router = self.router.fallback(handler);
        self.tree.nodes.push(ApiRouteNode::Fallback {
            source: SourceRange::default(),
        });
        self
    }

    #[must_use]
    pub fn into_router(self) -> Router<S> {
        self.router
    }

    #[must_use]
    pub fn into_route_tree(self) -> ApiRouteTree {
        self.tree
    }
}

#[doc(hidden)]
pub trait ApiRouteRegistration<H, T, S>
where
    S: Clone + Send + Sync + 'static,
{
    fn register(self, router: ApiRouter<S>, handler: H) -> ApiRouter<S>;
}

impl<S> ApiRouteRegistration<MethodRouter<S>, (), S> for &str
where
    S: Clone + Send + Sync + 'static,
{
    fn register(self, mut router: ApiRouter<S>, method_router: MethodRouter<S>) -> ApiRouter<S> {
        router.router = router.router.route(self, method_router);
        router.tree.nodes.push(ApiRouteNode::RawRoute {
            path: self.to_owned(),
            source: SourceRange::default(),
        });
        router
    }
}

impl<S> IntoApiRoot for ApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    fn into_api_module(self) -> api_core::ApiModule {
        self.into_route_tree().into_api_module()
    }

    fn into_route_tree(self) -> Option<ApiRouteTree> {
        Some(self.into_route_tree())
    }
}

#[must_use]
pub fn method_router<H, T, S>(method: HttpMethod, handler: H) -> MethodRouter<S>
where
    H: Handler<T, S>,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    match method {
        HttpMethod::Delete => delete(handler),
        HttpMethod::Get => get(handler),
        HttpMethod::Patch => patch(handler),
        HttpMethod::Post => post(handler),
        HttpMethod::Put => put(handler),
    }
}

/// JSON success response with a `200 OK` status.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Json<T>(pub T);

/// JSON success response with a `201 Created` status.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Created<T>(pub T);

/// Empty success response with a `204 No Content` status.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NoContent;

/// Server-sent event stream response.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Sse<T, S = T> {
    stream: S,
    _item: PhantomData<fn() -> T>,
}

/// Route path extractor for endpoint handlers.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Path<T>(pub T);

/// Query string extractor for endpoint handlers.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Query<T>(pub T);

/// JSON request body extractor for endpoint handlers.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Body<T>(pub T);

/// Raw binary request or response body for endpoint handlers.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Binary<T = Bytes>(pub T);

/// Wrapper for typed API domain errors.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DomainError<E>(pub E);

#[must_use]
pub fn success_or_error<T, E>(result: Result<T, E>) -> Response
where
    T: IntoResponse,
    E: ApiError + Serialize,
{
    match result {
        Ok(success) => success.into_response(),
        Err(error) => DomainError(error).into_response(),
    }
}

/// Success response conversion used by generated Axum endpoint adapters.
pub trait ApiSuccessResponse {
    fn into_api_response(self) -> Response;
}

#[must_use]
pub fn into_api_response<T>(response: T) -> Response
where
    T: ApiSuccessResponse,
{
    response.into_api_response()
}

#[must_use]
pub fn success_or_error_response<T, E>(result: Result<T, E>) -> Response
where
    T: ApiSuccessResponse,
    E: ApiError + Serialize,
{
    match result {
        Ok(success) => success.into_api_response(),
        Err(error) => DomainError(error).into_response(),
    }
}

impl<T> Json<T> {
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Created<T> {
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T, S> Sse<T, S> {
    #[must_use]
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            _item: PhantomData,
        }
    }

    #[must_use]
    pub fn into_inner(self) -> S {
        self.stream
    }
}

impl<T> Path<T> {
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Query<T> {
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Body<T> {
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Binary<T> {
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Json<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> Deref for Created<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T, S> Deref for Sse<T, S> {
    type Target = S;

    fn deref(&self) -> &Self::Target {
        &self.stream
    }
}

impl<T> Deref for Path<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> Deref for Query<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> Deref for Body<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> Deref for Binary<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> From<T> for Json<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> From<T> for Created<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> From<T> for Path<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> From<T> for Query<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> From<T> for Body<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> From<T> for Binary<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> IntoResponse for Json<T>
where
    T: Serialize,
{
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

impl<T> IntoResponse for Created<T>
where
    T: Serialize,
{
    fn into_response(self) -> Response {
        (StatusCode::CREATED, axum::Json(self.0)).into_response()
    }
}

impl IntoResponse for Binary<Bytes> {
    fn into_response(self) -> Response {
        (
            [(
                CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            )],
            self.0,
        )
            .into_response()
    }
}

impl IntoResponse for NoContent {
    fn into_response(self) -> Response {
        StatusCode::NO_CONTENT.into_response()
    }
}

impl<T, S, E> IntoResponse for Sse<T, S>
where
    S: Stream<Item = Result<T, E>> + Send + 'static,
    T: Serialize,
    E: ApiError + Serialize,
{
    fn into_response(self) -> Response {
        let events = self.stream.map(|item| match item {
            Ok(item) => Event::default().json_data(item),
            Err(error) => Event::default().event("api-error").json_data(ErrorFrame {
                status: error.status().0,
                body: error,
            }),
        });

        AxumSse::new(events).into_response()
    }
}

impl<E> IntoResponse for DomainError<E>
where
    E: ApiError + Serialize,
{
    fn into_response(self) -> Response {
        (status_code(self.0.status()), axum::Json(self.0)).into_response()
    }
}

impl<T> ApiSuccessResponse for Json<T>
where
    T: Serialize,
{
    fn into_api_response(self) -> Response {
        self.into_response()
    }
}

impl<T> ApiSuccessResponse for Created<T>
where
    T: Serialize,
{
    fn into_api_response(self) -> Response {
        self.into_response()
    }
}

impl ApiSuccessResponse for NoContent {
    fn into_api_response(self) -> Response {
        self.into_response()
    }
}

impl ApiSuccessResponse for () {
    fn into_api_response(self) -> Response {
        StatusCode::NO_CONTENT.into_response()
    }
}

impl ApiSuccessResponse for Binary<Bytes> {
    fn into_api_response(self) -> Response {
        self.into_response()
    }
}

impl<T, S, E> ApiSuccessResponse for Sse<T, S>
where
    S: Stream<Item = Result<T, E>> + Send + 'static,
    T: Serialize,
    E: ApiError + Serialize,
{
    fn into_api_response(self) -> Response {
        self.into_response()
    }
}

impl<T> ApiSuccessResponse for api_core::Json<T>
where
    T: Serialize,
{
    fn into_api_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

impl<T> ApiSuccessResponse for api_core::Created<T>
where
    T: Serialize,
{
    fn into_api_response(self) -> Response {
        (StatusCode::CREATED, axum::Json(self.0)).into_response()
    }
}

impl ApiSuccessResponse for api_core::NoContent {
    fn into_api_response(self) -> Response {
        StatusCode::NO_CONTENT.into_response()
    }
}

impl ApiSuccessResponse for api_core::Binary<Bytes> {
    fn into_api_response(self) -> Response {
        (
            [(
                CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            )],
            self.0,
        )
            .into_response()
    }
}

impl<T> ApiSuccessResponse for api_core::Sse<T>
where
    T: Serialize + Send + 'static,
{
    fn into_api_response(self) -> Response {
        let events = futures_util::stream::once(async move { Event::default().json_data(self.0) });

        AxumSse::new(events).into_response()
    }
}

impl<S, T> FromRequestParts<S> for Path<T>
where
    axum::extract::Path<T>: FromRequestParts<S>,
    S: Send + Sync,
{
    type Rejection = <axum::extract::Path<T> as FromRequestParts<S>>::Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        axum::extract::Path::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Path(value)| Self(value))
    }
}

impl<S, T> FromRequestParts<S> for Query<T>
where
    axum::extract::Query<T>: FromRequestParts<S>,
    S: Send + Sync,
{
    type Rejection = <axum::extract::Query<T> as FromRequestParts<S>>::Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        axum::extract::Query::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Query(value)| Self(value))
    }
}

impl<S, T> FromRequest<S> for Body<T>
where
    axum::Json<T>: FromRequest<S>,
    S: Send + Sync,
{
    type Rejection = <axum::Json<T> as FromRequest<S>>::Rejection;

    async fn from_request(
        request: axum::extract::Request,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        axum::Json::from_request(request, state)
            .await
            .map(|axum::Json(value)| Self(value))
    }
}

impl<S> FromRequest<S> for Binary<Bytes>
where
    Bytes: FromRequest<S>,
    S: Send + Sync,
{
    type Rejection = <Bytes as FromRequest<S>>::Rejection;

    async fn from_request(
        request: axum::extract::Request,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        Bytes::from_request(request, state).await.map(Self)
    }
}

#[must_use]
pub const fn status_code(status: HttpStatus) -> StatusCode {
    match StatusCode::from_u16(status.0) {
        Ok(status) => status,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn normalize_route_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            segment
                .strip_prefix(':')
                .map_or_else(|| segment.to_owned(), |param| format!("{{{param}}}"))
        })
        .collect::<Vec<_>>()
        .join("/")
}

impl<T> From<Infallible> for DomainError<T> {
    fn from(value: Infallible) -> Self {
        match value {}
    }
}

#[derive(Serialize)]
struct ErrorFrame<E> {
    status: u16,
    body: E,
}

#[cfg(test)]
mod tests {
    use api_core::{ir::HttpMethod, ApiType, ContractRegistry, Endpoint, EndpointDescriptor};
    use axum::{
        body::{to_bytes, Body as AxumBody},
        extract::Request,
        http::{Method, StatusCode},
        routing::get as axum_get,
    };
    use serde::{Deserialize, Serialize};
    use tower::ServiceExt;

    use super::*;

    #[derive(Clone, Debug, Deserialize, Serialize)]
    struct User {
        id: i64,
        filter: String,
        name: String,
    }

    impl ApiType for User {
        const RUST_NAME: &'static str = "User";
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    struct CreateUser {
        name: String,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(tag = "_tag")]
    enum GetUserError {
        NotFound { id: i64 },
    }

    impl ApiType for GetUserError {
        const RUST_NAME: &'static str = "GetUserError";
    }

    impl ApiError for GetUserError {
        fn status(&self) -> HttpStatus {
            match self {
                Self::NotFound { .. } => HttpStatus(404),
            }
        }

        fn error_def() -> api_core::ir::ErrorDef {
            api_core::ir::ErrorDef {
                id: Self::error_ref().id,
                rust_path: Self::rust_path(),
                rust_name: Self::RUST_NAME.to_owned(),
                ts_name: Self::TS_NAME.to_owned(),
                variants: Vec::new(),
                source: api_core::ir::SourceRange::default(),
            }
        }
    }

    async fn get_user(Path(id): Path<i64>, Query(filter): Query<Filter>) -> Response {
        success_or_error::<_, GetUserError>(Ok(Json(User {
            id,
            filter: filter.filter,
            name: "Ada".to_owned(),
        })))
    }

    async fn missing_user(Path(id): Path<i64>) -> Response {
        success_or_error::<Json<User>, _>(Err(GetUserError::NotFound { id }))
    }

    async fn create_user(Body(body): Body<CreateUser>) -> Created<User> {
        Created(User {
            id: 7,
            filter: String::new(),
            name: body.name,
        })
    }

    async fn download_avatar(Path(id): Path<i64>) -> Binary {
        Binary(Bytes::from(format!("avatar:{id}")))
    }

    async fn upload_avatar(Binary(bytes): Binary) -> Json<User> {
        Json(User {
            id: bytes.len() as i64,
            filter: String::new(),
            name: String::from_utf8(bytes.to_vec()).expect("utf8 upload"),
        })
    }

    type UserEventStream =
        futures_util::stream::Iter<std::array::IntoIter<Result<User, GetUserError>, 2>>;

    async fn user_events() -> Sse<User, UserEventStream> {
        Sse::new(futures_util::stream::iter([
            Ok(User {
                id: 1,
                filter: String::new(),
                name: "Ada".to_owned(),
            }),
            Err(GetUserError::NotFound { id: 2 }),
        ]))
    }

    #[derive(Clone, Debug, Deserialize)]
    struct Filter {
        filter: String,
    }

    fn noop_register(_registry: &mut ContractRegistry) {}

    fn descriptor(method: HttpMethod, path: &str) -> EndpointDescriptor {
        EndpointDescriptor::new(Endpoint::new(method, path), noop_register)
    }

    #[tokio::test]
    async fn registers_handlers_from_endpoint_metadata() {
        let app = ApiRouter::new("users")
            .endpoint_descriptor(
                descriptor(HttpMethod::Get, "/users/{id}"),
                get_user,
                SourceRange::default(),
            )
            .into_router();

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/users/42?filter=active")
                    .body(AxumBody::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(
            std::str::from_utf8(&body).expect("utf8"),
            r#"{"id":42,"filter":"active","name":"Ada"}"#
        );
    }

    #[tokio::test]
    async fn endpoint_descriptor_mounts_runtime_handler_and_contract_node() {
        let router = ApiRouter::new("users").endpoint_descriptor(
            descriptor(HttpMethod::Get, "/users/{id}"),
            get_user,
            api_core::ir::SourceRange::default(),
        );
        let module = router.clone().into_route_tree().into_api_module();

        assert_eq!(module.name(), "users");
        assert_eq!(module.endpoints().len(), 1);
        assert_eq!(module.endpoints()[0].route.0, "/users/{id}");

        let response = router
            .into_router()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/users/42?filter=active")
                    .body(AxumBody::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn nested_api_router_prefixes_collected_endpoint_paths() {
        let child = ApiRouter::new("users").endpoint_descriptor(
            descriptor(HttpMethod::Get, "/users/{id}"),
            get_user,
            api_core::ir::SourceRange::default(),
        );
        let router = ApiRouter::new("server").nest("/api", child);
        let module = router.clone().into_route_tree().into_api_module();

        assert_eq!(module.name(), "server");
        assert_eq!(module.endpoints().len(), 1);
        assert_eq!(module.endpoints()[0].route.0, "/api/users/{id}");

        let response = router
            .into_router()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/users/42?filter=active")
                    .body(AxumBody::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn raw_axum_routes_stay_runtime_only() {
        let router = ApiRouter::new("server").route(
            "/healthz",
            axum_get(|| async {
                Json(User {
                    id: 0,
                    filter: String::new(),
                    name: "ok".to_owned(),
                })
            }),
        );
        let module = router.clone().into_route_tree().into_api_module();

        assert!(module.endpoints().is_empty());

        let response = router
            .into_router()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/healthz")
                    .body(AxumBody::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn serializes_domain_errors_with_declared_status() {
        let app = ApiRouter::new("users")
            .endpoint_descriptor(
                descriptor(HttpMethod::Get, "/missing/{id}"),
                missing_user,
                SourceRange::default(),
            )
            .into_router();

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/missing/99")
                    .body(AxumBody::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(
            std::str::from_utf8(&body).expect("utf8"),
            r#"{"_tag":"NotFound","id":99}"#
        );
    }

    #[tokio::test]
    async fn decodes_json_body_and_maps_created_status() {
        let app = ApiRouter::new("users")
            .endpoint_descriptor(
                descriptor(HttpMethod::Post, "/users"),
                create_user,
                SourceRange::default(),
            )
            .into_router();

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/users")
                    .header("content-type", "application/json")
                    .body(AxumBody::from(r#"{"name":"Grace"}"#))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(
            std::str::from_utf8(&body).expect("utf8"),
            r#"{"id":7,"filter":"","name":"Grace"}"#
        );
    }

    #[tokio::test]
    async fn serializes_sse_items_and_domain_errors() {
        let app = ApiRouter::new("events")
            .endpoint_descriptor(
                descriptor(HttpMethod::Get, "/events"),
                user_events,
                SourceRange::default(),
            )
            .into_router();

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/events")
                    .body(AxumBody::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = std::str::from_utf8(&body).expect("utf8");

        assert!(body.contains(r#"data: {"id":1,"filter":"","name":"Ada"}"#));
        assert!(body.contains("event: api-error"));
        assert!(body.contains(r#"data: {"status":404,"body":{"_tag":"NotFound","id":2}}"#));
    }

    #[tokio::test]
    async fn binary_response_returns_raw_bytes() {
        let app = ApiRouter::new("files")
            .endpoint_descriptor(
                descriptor(HttpMethod::Get, "/avatars/{id}"),
                download_avatar,
                SourceRange::default(),
            )
            .into_router();

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/avatars/42")
                    .body(AxumBody::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            "application/octet-stream"
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(body.as_ref(), b"avatar:42");
    }

    #[tokio::test]
    async fn binary_request_extracts_raw_bytes() {
        let app = ApiRouter::new("files")
            .endpoint_descriptor(
                descriptor(HttpMethod::Post, "/avatars"),
                upload_avatar,
                SourceRange::default(),
            )
            .into_router();

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/avatars")
                    .header("content-type", "application/octet-stream")
                    .body(AxumBody::from("raw-avatar"))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(
            std::str::from_utf8(&body).expect("utf8"),
            r#"{"id":10,"filter":"","name":"raw-avatar"}"#
        );
    }
}
