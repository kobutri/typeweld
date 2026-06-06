//! Axum integration adapter.
//!
//! Use this crate's `Json`, `Path`, `Query`, `Body`, `Created`, `NoContent`,
//! and `Sse` wrappers in Axum handler signatures. The `#[api]` macro reads
//! those signatures into the same framework-neutral contract shapes used by
//! `api_core`.

use std::{convert::Infallible, marker::PhantomData, ops::Deref};

use api_core::{
    ir::{HttpMethod, HttpStatus},
    ApiError, ApiModule, Endpoint,
};
use axum::{
    extract::{FromRequest, FromRequestParts},
    handler::Handler,
    http::{request::Parts, StatusCode},
    response::{
        sse::{Event, Sse as AxumSse},
        IntoResponse, Response,
    },
    routing::{delete, get, patch, post, put, MethodRouter},
    Router,
};
use futures_util::{Stream, StreamExt};
use serde::Serialize;

/// Axum router builder paired with an explicit API module.
///
/// The module supplies the exported endpoint metadata; each route call pairs
/// one endpoint descriptor with the concrete Axum handler that serves it.
#[derive(Clone, Debug)]
pub struct ApiRouter<S = ()> {
    module: ApiModule,
    router: Router<S>,
}

impl ApiRouter<()> {
    #[must_use]
    pub fn new(module: ApiModule) -> Self {
        Self {
            module,
            router: Router::new(),
        }
    }
}

impl<S> ApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    #[must_use]
    pub fn with_state<T>(self, state: S) -> ApiRouter<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        ApiRouter {
            module: self.module,
            router: self.router.with_state(state),
        }
    }

    /// Register a handler using the endpoint descriptor's declared method and route.
    #[must_use]
    pub fn route<H, T>(mut self, endpoint: Endpoint, handler: H) -> Self
    where
        H: Handler<T, S>,
        T: 'static,
    {
        let Endpoint { method, route, .. } = endpoint;
        self.router = self.router.route(
            &normalize_route_path(&route.0),
            method_router(method, handler),
        );
        self
    }

    /// Register a handler under an explicitly overridden method and route.
    ///
    /// Prefer [`Self::route`] for ordinary API endpoints. This helper is for
    /// deliberate adapter-level exceptions where the Axum mount point is not
    /// the contract route.
    #[must_use]
    pub fn route_override<H, T>(
        mut self,
        _endpoint: Endpoint,
        method: HttpMethod,
        path: impl AsRef<str>,
        handler: H,
    ) -> Self
    where
        H: Handler<T, S>,
        T: 'static,
    {
        self.router = self.router.route(
            &normalize_route_path(path.as_ref()),
            method_router(method, handler),
        );
        self
    }

    #[must_use]
    pub fn module(&self) -> &ApiModule {
        &self.module
    }

    #[must_use]
    pub fn into_router(self) -> Router<S> {
        self.router
    }
}

#[must_use]
pub fn router(module: ApiModule) -> ApiRouter {
    ApiRouter::new(module)
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
    use api_core::{ir::HttpMethod, ApiType};
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

    #[tokio::test]
    async fn registers_handlers_from_endpoint_metadata() {
        let module = ApiModule::new("users");
        let endpoint = Endpoint::new(HttpMethod::Get, "/users/{id}");
        let app = router(module).route(endpoint, get_user).into_router();

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
    async fn route_uses_descriptor_path_and_method() {
        let endpoint = Endpoint::new(HttpMethod::Post, "/users");
        let app = router(ApiModule::new("users"))
            .route(endpoint, create_user)
            .into_router();

        let response = app
            .clone()
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

        let wrong_method = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/users")
                    .body(AxumBody::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(wrong_method.status(), StatusCode::METHOD_NOT_ALLOWED);

        let wrong_path = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/people")
                    .header("content-type", "application/json")
                    .body(AxumBody::from(r#"{"name":"Grace"}"#))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(wrong_path.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn route_override_makes_contract_divergence_explicit() {
        let endpoint = Endpoint::new(HttpMethod::Post, "/contract-users");
        let app = router(ApiModule::new("users"))
            .route_override(endpoint, HttpMethod::Get, "/legacy-users/{id}", get_user)
            .into_router();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/legacy-users/42?filter=active")
                    .body(AxumBody::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);

        let contract_route = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/contract-users")
                    .body(AxumBody::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(contract_route.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn serializes_domain_errors_with_declared_status() {
        let app = router(ApiModule::new("users"))
            .route(Endpoint::new(HttpMethod::Get, "/missing/:id"), missing_user)
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
        let app = router(ApiModule::new("users"))
            .route(Endpoint::new(HttpMethod::Post, "/users"), create_user)
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
        let app = router(ApiModule::new("events"))
            .route(Endpoint::new(HttpMethod::Get, "/events"), user_events)
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
}
