#[derive(typeweld_macros::ApiType)]
struct User {
    id: i64,
}

#[typeweld_macros::api(method = "GET", path = "/download")]
async fn download_file() -> typeweld_core::Binary<User> {
    unimplemented!()
}

fn main() {}
