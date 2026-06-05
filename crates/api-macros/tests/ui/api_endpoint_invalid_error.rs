use api_core::Json;

#[derive(api_macros::ApiType)]
struct User {
    id: i64,
}

struct NotApiError;

#[api_macros::api(method = "GET", path = "/users")]
async fn get_user() -> Result<Json<User>, NotApiError> {
    unimplemented!()
}

fn main() {}
