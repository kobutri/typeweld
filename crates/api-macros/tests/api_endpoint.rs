use api_core::{
    ir::{HttpMethod, RequestBodyTransport, ResponseShape, Transport},
    ApiType, Binary, Created, Json, Path, Query, Sse,
};
use axum::{body::to_bytes, http::StatusCode};

#[derive(api_macros::ApiType, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct User {
    id: i64,
}

#[derive(api_macros::ApiError, serde::Serialize)]
#[serde(tag = "_tag")]
#[allow(dead_code)]
enum GetUserError {
    #[api_error(status = 404)]
    NotFound,
}

#[derive(api_macros::ApiError, serde::Serialize)]
#[serde(tag = "_tag")]
#[allow(dead_code)]
enum SearchUsersError {
    #[api_error(status = 400)]
    BadFilter,
}

#[api_macros::api(method = "GET", path = "/users/{id}", allow_unused)]
#[allow(dead_code)]
async fn get_user(id: Path<i64>, filter: Query<String>) -> Result<Json<User>, GetUserError> {
    let _ = (id, filter);
    unimplemented!()
}

#[api_macros::api(method = "POST", path = "/users")]
#[allow(dead_code)]
async fn create_user() -> Result<Created<User>, GetUserError> {
    unimplemented!()
}

#[api_macros::api(method = "SSE", path = "/users/events")]
#[allow(dead_code)]
fn user_events() -> Result<Sse<User>, GetUserError> {
    unimplemented!()
}

#[api_macros::api(method = "GET", path = "/users/search")]
#[allow(dead_code)]
async fn search_users() -> Result<Json<User>, SearchUsersError> {
    unimplemented!()
}

#[api_macros::api(method = "POST", path = "/axum-users/{id}")]
#[allow(dead_code)]
async fn axum_wrapped_create_user(
    id: api_axum::Path<i64>,
    body: api_axum::Body<User>,
) -> Result<api_axum::Created<User>, GetUserError> {
    let _ = (id, body);
    std::future::ready(()).await;
    unimplemented!()
}

type UserEventStream =
    futures_util::stream::Iter<std::array::IntoIter<Result<User, GetUserError>, 1>>;

#[api_macros::api(method = "SSE", path = "/axum-users/events")]
#[allow(dead_code)]
fn axum_wrapped_user_events() -> Result<api_axum::Sse<User, UserEventStream>, GetUserError> {
    unimplemented!()
}

#[api_macros::api(method = "GET", path = "/axum-users/plain")]
#[allow(dead_code)]
async fn axum_plain_user() -> api_axum::Json<User> {
    api_axum::Json(User { id: 7 })
}

#[api_macros::api(method = "GET", path = "/axum-users/result")]
#[allow(dead_code)]
async fn axum_result_user() -> Result<api_axum::Json<User>, GetUserError> {
    Ok(api_axum::Json(User { id: 9 }))
}

#[api_macros::api(method = "GET", path = "/axum-users/missing")]
#[allow(dead_code)]
async fn axum_missing_user() -> Result<api_axum::Json<User>, GetUserError> {
    Err(GetUserError::NotFound)
}

#[api_macros::api(method = "SSE", path = "/axum-users/plain-events")]
#[allow(dead_code)]
fn axum_plain_events() -> api_axum::Sse<User, UserEventStream> {
    api_axum::Sse::new(futures_util::stream::iter([Ok(User { id: 11 })]))
}

#[api_macros::api(method = "SSE", path = "/axum-users/result-events")]
#[allow(dead_code)]
fn axum_result_events() -> Result<api_axum::Sse<User, UserEventStream>, GetUserError> {
    Ok(api_axum::Sse::new(futures_util::stream::iter([Ok(User {
        id: 13,
    })])))
}

#[api_macros::api(method = "GET", path = "/files/{id}")]
#[allow(dead_code)]
async fn download_file(id: Path<i64>) -> Result<Binary<bytes::Bytes>, GetUserError> {
    let _ = id;
    std::future::ready(()).await;
    unimplemented!()
}

#[api_macros::api(method = "GET", path = "/axum-files/{id}")]
#[allow(dead_code)]
async fn axum_download_file(id: api_axum::Path<i64>) -> api_axum::Binary<bytes::Bytes> {
    let _ = id;
    api_axum::Binary(bytes::Bytes::from_static(b"file"))
}

#[api_macros::api(method = "POST", path = "/axum-files/{id}")]
#[allow(dead_code)]
async fn axum_upload_file(
    id: api_axum::Path<i64>,
    body: api_axum::Binary<bytes::Bytes>,
) -> api_axum::Json<User> {
    let _ = (id, body);
    api_axum::Json(User { id: 17 })
}

#[api_macros::api(method = "POST", path = "/files/{id}")]
#[allow(dead_code)]
async fn upload_file(
    id: Path<i64>,
    body: Binary<bytes::Bytes>,
) -> Result<Json<User>, GetUserError> {
    let _ = (id, body);
    std::future::ready(()).await;
    unimplemented!()
}

#[test]
fn api_endpoint_macro_emits_metadata() {
    let endpoint = __api_endpoint_get_user();

    assert_eq!(endpoint.method, HttpMethod::Get);
    assert_eq!(endpoint.route.0, "/users/{id}");
    assert_eq!(endpoint.rust_name, "get_user");
    assert_eq!(endpoint.ts_path, ["users", "getUser"]);
    assert!(endpoint.allow_unused);
    assert_eq!(endpoint.request.path_params[0].rust_name, "id");
    assert_eq!(endpoint.request.query_params[0].rust_name, "filter");
    assert_eq!(endpoint.errors[0].name, "GetUserError");

    let ResponseShape::Json(response) = endpoint.response else {
        panic!("expected json response");
    };
    assert_eq!(response.name, "User");
}

#[test]
fn api_endpoint_macro_emits_sse_metadata() {
    let endpoint = __api_endpoint_user_events();

    assert_eq!(endpoint.method, HttpMethod::Get);
    assert_eq!(endpoint.transport, Transport::ServerSentEvents);
    assert_eq!(endpoint.route.0, "/users/events");
    assert_eq!(endpoint.ts_path, ["users", "userEvents"]);
    assert_eq!(endpoint.errors[0].name, "GetUserError");

    let ResponseShape::Stream(item) = endpoint.response else {
        panic!("expected stream response");
    };
    assert_eq!(item.name, "User");
}

#[test]
fn api_endpoint_macro_preserves_created_response_shape() {
    let endpoint = __api_endpoint_create_user();

    assert_eq!(endpoint.method, HttpMethod::Post);
    let ResponseShape::Created(response) = endpoint.response else {
        panic!("expected created response");
    };
    assert_eq!(response.name, "User");
}

#[test]
fn api_endpoint_macro_accepts_endpoint_specific_error_enum() {
    let endpoint = __api_endpoint_search_users();

    assert_eq!(endpoint.errors[0].name, "SearchUsersError");
    assert_eq!(endpoint.ts_path, ["users", "searchUsers"]);
}

#[test]
fn api_endpoint_macro_accepts_axum_handler_wrappers() {
    let endpoint = __api_endpoint_axum_wrapped_create_user();

    assert_eq!(endpoint.method, HttpMethod::Post);
    assert_eq!(endpoint.route.0, "/axum-users/{id}");
    assert_eq!(endpoint.request.path_params[0].rust_name, "id");
    assert_eq!(endpoint.request.body.expect("body").name, "User");
    let ResponseShape::Created(response) = endpoint.response else {
        panic!("expected created response");
    };
    assert_eq!(response.name, "User");
}

#[test]
fn api_endpoint_macro_accepts_axum_sse_wrapper() {
    let endpoint = __api_endpoint_axum_wrapped_user_events();

    assert_eq!(endpoint.method, HttpMethod::Get);
    assert_eq!(endpoint.transport, Transport::ServerSentEvents);
    let ResponseShape::Stream(item) = endpoint.response else {
        panic!("expected stream response");
    };
    assert_eq!(item.name, "User");
}

#[test]
fn api_endpoint_macro_emits_binary_download_metadata() {
    let endpoint = __api_endpoint_download_file();

    assert_eq!(endpoint.method, HttpMethod::Get);
    assert_eq!(endpoint.transport, Transport::BinaryDownload);
    assert_eq!(endpoint.route.0, "/files/{id}");
    let ResponseShape::Binary { content_type } = endpoint.response else {
        panic!("expected binary response");
    };
    assert_eq!(content_type, None);
}

#[test]
fn api_endpoint_macro_emits_binary_upload_metadata() {
    let endpoint = __api_endpoint_upload_file();

    assert_eq!(endpoint.method, HttpMethod::Post);
    assert_eq!(endpoint.transport, Transport::BinaryUpload);
    assert_eq!(
        endpoint.request.body_transport,
        RequestBodyTransport::Binary
    );
    assert_eq!(endpoint.request.body.expect("body").name, "Bytes");
    let ResponseShape::Json(response) = endpoint.response else {
        panic!("expected json response");
    };
    assert_eq!(response.name, "User");
}

#[test]
fn api_endpoint_macro_registers_reachable_types_and_errors() {
    let mut registry = api_core::ContractRegistry::new();

    __api_register_endpoint_get_user(&mut registry);
    __api_register_endpoint_get_user(&mut registry);

    let types = registry.type_defs();
    let errors = registry.error_defs();

    assert_eq!(types.len(), 1);
    assert_eq!(types[0].rust_name, "User");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].rust_name, "GetUserError");
}

#[test]
fn api_module_accepts_endpoint_functions_directly() {
    let module = api_core::api_module!(name = "users", endpoints = [get_user]);

    assert_eq!(module.endpoints.len(), 1);
    assert_eq!(module.endpoints[0].rust_name, "get_user");
    assert_eq!(module.endpoints[0].ts_path, ["users", "getUser"]);
    assert_eq!(module.registry().type_defs()[0].rust_name, "User");
    assert_eq!(module.registry().error_defs()[0].rust_name, "GetUserError");
}

#[test]
fn api_endpoint_macro_emits_axum_handler_adapters() {
    fn accepts_handler<H, T>(_: H)
    where
        H: axum::handler::Handler<T, ()> + Clone + Send + Sync + 'static,
        T: 'static,
    {
    }

    accepts_handler(__api_axum_handler_axum_wrapped_create_user);
    accepts_handler(__api_axum_handler_axum_plain_user);
    accepts_handler(__api_axum_handler_axum_result_user);
    accepts_handler(__api_axum_handler_axum_plain_events);
    accepts_handler(__api_axum_handler_axum_result_events);
    accepts_handler(__api_axum_handler_axum_download_file);
    accepts_handler(__api_axum_handler_axum_upload_file);
}

#[tokio::test]
async fn generated_axum_adapter_converts_result_success() {
    let response = __api_axum_handler_axum_result_user().await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    assert_eq!(std::str::from_utf8(&body).expect("utf8"), r#"{"id":9}"#);
}

#[tokio::test]
async fn generated_axum_adapter_converts_result_error() {
    let response = __api_axum_handler_axum_missing_user().await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    assert_eq!(
        std::str::from_utf8(&body).expect("utf8"),
        r#"{"_tag":"NotFound"}"#
    );
}

#[tokio::test]
async fn generated_axum_adapter_converts_plain_response() {
    let response = __api_axum_handler_axum_plain_user().await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    assert_eq!(std::str::from_utf8(&body).expect("utf8"), r#"{"id":7}"#);
}
