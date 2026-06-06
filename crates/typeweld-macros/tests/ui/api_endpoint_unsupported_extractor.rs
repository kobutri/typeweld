#[derive(typeweld_macros::ApiType)]
struct User {
    id: i64,
}

#[typeweld_macros::api(method = "GET", path = "/users")]
async fn get_user(raw_id: i64) -> typeweld_core::Json<User> {
    let _ = raw_id;
    unimplemented!()
}

fn main() {}
