#[derive(api_macros::ApiType)]
struct User {
    id: i64,
}

#[api_macros::api(method = "GET", path = "/users/events")]
async fn user_events() -> api_core::Sse<User> {
    unimplemented!()
}

fn main() {}
