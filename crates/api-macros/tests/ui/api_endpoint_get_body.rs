#[derive(api_macros::ApiType)]
struct User {
    id: i64,
}

#[api_macros::api(method = "GET", path = "/users")]
async fn get_user(body: api_core::Body<User>) -> api_core::Json<User> {
    let _ = body;
    unimplemented!()
}

fn main() {}
