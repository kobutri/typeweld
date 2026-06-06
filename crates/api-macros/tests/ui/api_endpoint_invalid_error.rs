use api_core::Json;

#[derive(api_macros::ApiType, serde::Serialize)]
struct User {
    id: i64,
}

#[derive(serde::Serialize)]
struct NotApiError;

#[api_macros::api(method = "GET", path = "/users")]
async fn get_user() -> Result<Json<User>, NotApiError> {
    unimplemented!()
}

fn main() {}
