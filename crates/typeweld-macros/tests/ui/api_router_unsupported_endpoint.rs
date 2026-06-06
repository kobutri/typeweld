#[derive(typeweld_macros::ApiType, serde::Serialize)]
struct User {
    id: i64,
}

#[typeweld_macros::api(method = "GET", path = "/users")]
async fn get_user() -> typeweld_axum::Json<User> {
    typeweld_axum::Json(User { id: 1 })
}

#[typeweld_macros::api_router]
fn routes() -> typeweld_axum::ApiRouter {
    typeweld_axum::ApiRouter::new("users")
        .endpoint(|| async { typeweld_axum::Json(User { id: 1 }) })
        .endpoint(get_user())
}

fn main() {}
