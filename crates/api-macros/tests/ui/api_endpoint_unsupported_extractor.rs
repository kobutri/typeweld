#[derive(api_macros::ApiType)]
struct User {
    id: i64,
}

#[api_macros::api(method = "GET", path = "/users")]
async fn get_user(raw_id: i64) -> api_core::Json<User> {
    let _ = raw_id;
    unimplemented!()
}

fn main() {}
