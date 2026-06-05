use api_core::{Json, Path};

#[derive(api_macros::ApiType)]
struct User {
    id: i64,
}

#[api_macros::api(method = "GET", path = "/users/{id}")]
async fn get_user(other: Path<i64>) -> Json<User> {
    let _ = other;
    unimplemented!()
}

fn main() {}
