#[derive(api_macros::ApiType)]
struct User {
    id: i64,
}

#[api_macros::api(method = "SSE", path = "/users/events")]
fn user_events() -> api_core::Json<User> {
    unimplemented!()
}

fn main() {}
