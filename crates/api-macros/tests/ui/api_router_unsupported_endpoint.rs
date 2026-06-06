#[derive(api_macros::ApiType, serde::Serialize)]
struct User {
    id: i64,
}

#[api_macros::api(method = "GET", path = "/users")]
async fn get_user() -> api_axum::Json<User> {
    api_axum::Json(User { id: 1 })
}

#[api_macros::api_router]
fn routes() -> api_axum::ApiRouter {
    api_axum::ApiRouter::new("users")
        .endpoint(|| async { api_axum::Json(User { id: 1 }) })
        .endpoint(get_user())
}

fn main() {}
