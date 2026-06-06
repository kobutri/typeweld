#[derive(typeweld_macros::ApiType)]
struct User {
    id: i64,
}

#[typeweld_macros::api(method = "GET", path = "/users")]
async fn get_user(id: typeweld_core::Path<i64>) -> typeweld_core::Json<User> {
    let _ = id;
    unimplemented!()
}

fn main() {}
