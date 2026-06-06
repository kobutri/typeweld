#[derive(typeweld_macros::ApiType)]
struct User {
    id: i64,
}

#[typeweld_macros::api(method = "GET", path = "/users")]
async fn get_user() -> Result<typeweld_core::Json<User>, Box<dyn std::error::Error>> {
    unimplemented!()
}

fn main() {}
