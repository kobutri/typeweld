#[derive(typeweld_macros::ApiType)]
struct User {
    id: i64,
}

#[typeweld_macros::api(method = "GET", path = "/users/events")]
async fn user_events() -> typeweld_core::Sse<User> {
    unimplemented!()
}

fn main() {}
