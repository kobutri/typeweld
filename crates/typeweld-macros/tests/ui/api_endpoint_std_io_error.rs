#[derive(typeweld_macros::ApiType)]
struct User {
    id: i64,
}

#[typeweld_macros::api(method = "GET", path = "/users")]
async fn get_user() -> Result<typeweld_core::Json<User>, std::io::Error> {
    unimplemented!()
}

fn main() {}
