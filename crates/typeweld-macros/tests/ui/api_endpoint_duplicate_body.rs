#[derive(typeweld_macros::ApiType)]
struct User {
    id: i64,
}

#[typeweld_macros::api(method = "POST", path = "/users")]
async fn create_user(
    first: typeweld_core::Body<User>,
    second: typeweld_core::Body<User>,
) -> typeweld_core::Json<User> {
    let _ = (first, second);
    unimplemented!()
}

fn main() {}
