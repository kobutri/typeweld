use typeweld_core::Json;

#[derive(typeweld_macros::ApiType)]
struct User {
    id: i64,
}

#[typeweld_macros::api(method = "GET", path = "/users")]
fn get_user() -> Json<User> {
    unimplemented!()
}

fn main() {}
