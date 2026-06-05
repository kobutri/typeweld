use api_core::{
    ir::{HttpMethod, ResponseShape},
    ApiType, Json, Path, Query,
};

#[derive(api_macros::ApiType)]
#[allow(dead_code)]
struct User {
    id: i64,
}

#[derive(api_macros::ApiError)]
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

#[test]
fn api_endpoint_macro_emits_metadata() {
    let endpoint = __api_endpoint_get_user();

    assert_eq!(endpoint.method, HttpMethod::Get);
    assert_eq!(endpoint.route.0, "/users/{id}");
    assert_eq!(endpoint.rust_name, "get_user");
    assert!(endpoint.allow_unused);
    assert_eq!(endpoint.request.path_params[0].rust_name, "id");
    assert_eq!(endpoint.request.query_params[0].rust_name, "filter");
    assert_eq!(endpoint.errors[0].name, "GetUserError");

    let ResponseShape::Json(response) = endpoint.response else {
        panic!("expected json response");
    };
    assert_eq!(response.name, "User");
}
