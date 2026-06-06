#[derive(typeweld_macros::ApiType)]
struct User {
    id: i64,
}

#[typeweld_macros::api(method = "GET", path = "/users")]
async fn get_user(body: typeweld_core::Body<User>) -> typeweld_core::Json<User> {
    let _ = body;
    unimplemented!()
}

fn main() {}
