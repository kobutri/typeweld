use api_core::{
    ir::{HttpMethod, RequestBodyTransport, ResponseShape, Transport},
    ApiType, Binary, Created, Json, Path, Query, Sse,
};

#[derive(api_macros::ApiType)]
#[allow(dead_code)]
struct User {
    id: i64,
}

#[derive(api_macros::ApiError)]
#[serde(tag = "_tag")]
#[allow(dead_code)]
enum GetUserError {
    #[api_error(status = 404)]
    NotFound,
}

#[derive(api_macros::ApiError)]
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

#[api_macros::api(method = "SSE", path = "/axum-users/events")]
#[allow(dead_code)]
fn axum_wrapped_user_events() -> Result<api_axum::Sse<User>, GetUserError> {
    unimplemented!()
}

#[api_macros::api(method = "GET", path = "/files/{id}")]
#[allow(dead_code)]
async fn download_file(id: Path<i64>) -> Result<Binary<bytes::Bytes>, GetUserError> {
    let _ = id;
    std::future::ready(()).await;
    unimplemented!()
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
