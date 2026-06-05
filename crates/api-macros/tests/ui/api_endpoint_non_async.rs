use api_core::Json;

#[derive(api_macros::ApiType)]
struct User {
    id: i64,
}

#[api_macros::api(method = "GET", path = "/users")]
fn get_user() -> Json<User> {
    unimplemented!()
}

fn main() {}
