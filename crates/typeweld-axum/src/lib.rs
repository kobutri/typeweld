//! Axum runtime integration for typeweld APIs.
//!
//! This crate contains only runtime types: extractor and response wrappers
//! for handler signatures, the [`ApiRouter`] builder, and the [`ApiBound`] /
//! [`ApiStatus`] traits implemented by the derive macros. It carries no
//! contract metadata — the static extractor reads that from source.

use std::{convert::Infallible, marker::PhantomData, ops::Deref};

pub use axum::response::Response;
pub use bytes::Bytes;

use axum::{
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

/// Marker trait for types that may cross the API boundary.
///
/// Implemented by `#[derive(Api)]` / `#[derive(ApiError)]` and provided for
/// primitives, common containers, and feature-gated external types. The
/// derive macros emit an `ApiBound` assertion for every field, parameter, and
/// response payload, so code that compiles is guaranteed to reference only
/// types the static extractor can also resolve.
pub trait ApiBound {}

macro_rules! primitive_api_bound {
    ($($ty:ty),* $(,)?) => {
        $(impl ApiBound for $ty {})*
    };
}

primitive_api_bound!(
    bool, i8, u8, i16, u16, i32, u32, i64, u64, i128, u128, usize, isize, f32, f64, String,
);

impl<T: ApiBound> ApiBound for Option<T> {}
impl<T: ApiBound> ApiBound for Vec<T> {}
impl<T: ApiBound> ApiBound for Box<T> {}
impl<T: ApiBound> ApiBound for std::collections::HashMap<String, T> {}
impl<T: ApiBound> ApiBound for std::collections::BTreeMap<String, T> {}

#[cfg(feature = "uuid")]
impl ApiBound for uuid::Uuid {}
#[cfg(feature = "chrono")]
impl ApiBound for chrono::DateTime<chrono::Utc> {}
#[cfg(feature = "decimal")]
impl ApiBound for rust_decimal::Decimal {}
#[cfg(feature = "json")]
impl ApiBound for serde_json::Value {}

/// HTTP status access for API error enums, implemented by
/// `#[derive(ApiError)]` from the per-variant `#[status(...)]` attributes.
pub trait ApiStatus {
    fn status(&self) -> u16;
}

/// HTTP methods supported by typeweld endpoints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpMethod {
    Delete,
    Get,
    Patch,
    Post,
    Put,
}

/// A route plus its handler, produced by the `#[api]` macro's mount helper
/// and consumed by [`ApiRouter::endpoint`] (via `#[api_router]` rewriting).
pub struct EndpointMount<S = ()> {
    pub path: &'static str,
    pub method_router: MethodRouter<S>,
}

/// Builds an Axum router from typeweld endpoints while passing through all
/// regular Axum composition (state, layers, fallbacks, raw routes).
#[derive(Clone, Debug)]
pub struct ApiRouter<S = ()> {
    router: Router<S>,
}

impl Default for ApiRouter<()> {
    fn default() -> Self {
        Self::new()
    }
}

impl ApiRouter<()> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            router: Router::new(),
        }
    }
}

impl<S> ApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    /// Mounts an `#[api]` endpoint.
    ///
    /// Written as `.endpoint(handler_fn)` inside an `#[api_router]` function;
    /// the macro rewrites the argument to the generated mount helper.
    #[must_use]
    pub fn endpoint(mut self, mount: EndpointMount<S>) -> Self {
        self.router = self
            .router
            .route(&normalize_route_path(mount.path), mount.method_router);
        self
    }

    #[must_use]
    pub fn with_state<S2>(self, state: S) -> ApiRouter<S2>
    where
        S2: Clone + Send + Sync + 'static,
    {
        ApiRouter {
            router: self.router.with_state(state),
        }
    }

    /// Adds a raw Axum route, invisible to the generated client.
    #[must_use]
    pub fn route(mut self, path: &str, method_router: MethodRouter<S>) -> Self {
        self.router = self.router.route(path, method_router);
        self
    }

    #[must_use]
    pub fn route_service<T>(mut self, path: &str, service: T) -> Self
    where
        T: Service<Request, Error = Infallible> + Clone + Send + Sync + 'static,
        T::Response: IntoResponse,
        T::Future: Send + 'static,
    {
        self.router = self.router.route_service(path, service);
        self
    }

    #[must_use]
    pub fn nest(mut self, path: &str, router: ApiRouter<S>) -> Self {
        self.router = self.router.nest(path, router.router);
        self
    }

    #[must_use]
    pub fn merge(mut self, router: ApiRouter<S>) -> Self {
        self.router = self.router.merge(router.router);
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
        self.router = self.router.layer(layer);
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
        self.router = self.router.route_layer(layer);
        self
    }

    #[must_use]
    pub fn fallback<H, T>(mut self, handler: H) -> Self
    where
        H: Handler<T, S>,
        T: 'static,
    {
        self.router = self.router.fallback(handler);
        self
    }

    /// Finishes the builder, returning the underlying Axum router.
    #[must_use]
    pub fn into_router(self) -> Router<S> {
        self.router
    }
}

impl<S> From<ApiRouter<S>> for Router<S> {
    fn from(router: ApiRouter<S>) -> Self {
        router.router
    }
}

/// Builds a method router for the generated mount helpers.
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
    E: ApiStatus + Serialize,
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
    E: ApiStatus + Serialize,
{
    fn into_response(self) -> Response {
        let events = self.stream.map(|item| match item {
            Ok(item) => Event::default().json_data(item),
            Err(error) => Event::default().event("api-error").json_data(ErrorFrame {
                status: error.status(),
                body: error,
            }),
        });

        AxumSse::new(events).into_response()
    }
}

impl<E> IntoResponse for DomainError<E>
where
    E: ApiStatus + Serialize,
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
    E: ApiStatus + Serialize,
{
    fn into_api_response(self) -> Response {
        self.into_response()
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
pub const fn status_code(status: u16) -> StatusCode {
    match StatusCode::from_u16(status) {
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
    use axum::{
        body::{to_bytes, Body as AxumBody},
        extract::Request,
        http::{Method, StatusCode},
    };
    use serde::{Deserialize, Serialize};
    use tower::ServiceExt;

    use super::*;

    #[derive(Clone, Debug, Deserialize, Serialize)]
    struct User {
        id: i64,
        name: String,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(tag = "_tag")]
    enum GetUserError {
        NotFound { id: i64 },
    }

    impl ApiStatus for GetUserError {
        fn status(&self) -> u16 {
            match self {
                Self::NotFound { .. } => 404,
            }
        }
    }

    async fn get_user(Path(id): Path<i64>) -> Response {
        success_or_error_response::<_, GetUserError>(Ok(Json(User {
            id,
            name: "Ada".to_owned(),
        })))
    }

    async fn missing_user(Path(id): Path<i64>) -> Response {
        success_or_error_response::<Json<User>, _>(Err(GetUserError::NotFound { id }))
    }

    type UserEventStream =
        futures_util::stream::Iter<std::array::IntoIter<Result<User, GetUserError>, 2>>;

    async fn user_events() -> Response {
        into_api_response(Sse::<User, UserEventStream>::new(
            futures_util::stream::iter([
                Ok(User {
                    id: 1,
                    name: "Ada".to_owned(),
                }),
                Err(GetUserError::NotFound { id: 2 }),
            ]),
        ))
    }

    fn mount<S>(path: &'static str, router: MethodRouter<S>) -> EndpointMount<S> {
        EndpointMount {
            path,
            method_router: router,
        }
    }

    #[tokio::test]
    async fn endpoint_mounts_serve_requests() {
        let app = ApiRouter::new()
            .endpoint(mount(
                "/users/{id}",
                method_router(HttpMethod::Get, get_user),
            ))
            .into_router();

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/users/42")
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
            r#"{"id":42,"name":"Ada"}"#
        );
    }

    #[tokio::test]
    async fn domain_errors_use_declared_status_and_tagged_body() {
        let app = ApiRouter::new()
            .endpoint(mount(
                "/missing/{id}",
                method_router(HttpMethod::Get, missing_user),
            ))
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
    async fn sse_streams_items_and_error_frames() {
        let app = ApiRouter::new()
            .endpoint(mount(
                "/events",
                method_router(HttpMethod::Get, user_events),
            ))
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

        assert!(body.contains(r#"data: {"id":1,"name":"Ada"}"#));
        assert!(body.contains("event: api-error"));
        assert!(body.contains(r#"data: {"status":404,"body":{"_tag":"NotFound","id":2}}"#));
    }

    #[tokio::test]
    async fn legacy_colon_route_params_are_normalized() {
        let app = ApiRouter::new()
            .endpoint(mount(
                "/users/:id",
                method_router(HttpMethod::Get, get_user),
            ))
            .into_router();

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/users/7")
                    .body(AxumBody::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
    }
}
