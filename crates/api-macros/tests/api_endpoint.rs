use api_core::{
    ir::{HttpMethod, ResponseShape, Transport},
    ApiType, Created, Json, Path, Query, Sse,
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
