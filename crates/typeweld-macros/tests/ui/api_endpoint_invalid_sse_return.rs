#[derive(typeweld_macros::ApiType)]
struct User {
    id: i64,
}

#[typeweld_macros::api(method = "SSE", path = "/users/events")]
fn user_events() -> typeweld_core::Json<User> {
    unimplemented!()
}

fn main() {}
