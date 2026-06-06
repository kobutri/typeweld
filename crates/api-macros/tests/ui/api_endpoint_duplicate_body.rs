#[derive(api_macros::ApiType)]
struct User {
    id: i64,
}

#[api_macros::api(method = "POST", path = "/users")]
async fn create_user(
    first: api_core::Body<User>,
    second: api_core::Body<User>,
) -> api_core::Json<User> {
    let _ = (first, second);
    unimplemented!()
}

fn main() {}
