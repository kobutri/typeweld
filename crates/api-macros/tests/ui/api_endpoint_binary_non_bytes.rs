#[derive(api_macros::ApiType)]
struct User {
    id: i64,
}

#[api_macros::api(method = "GET", path = "/download")]
async fn download_file() -> api_core::Binary<User> {
    unimplemented!()
}

fn main() {}
