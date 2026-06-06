#[derive(api_macros::ApiType)]
struct User {
    id: i64,
}

#[api_macros::api(method = "GET", path = "/users")]
async fn get_user() -> Result<api_core::Json<User>, std::io::Error> {
    unimplemented!()
}

fn main() {}
