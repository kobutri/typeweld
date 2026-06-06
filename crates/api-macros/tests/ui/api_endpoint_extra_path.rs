#[derive(api_macros::ApiType)]
struct User {
    id: i64,
}

#[api_macros::api(method = "GET", path = "/users")]
async fn get_user(id: api_core::Path<i64>) -> api_core::Json<User> {
    let _ = id;
    unimplemented!()
}

fn main() {}
