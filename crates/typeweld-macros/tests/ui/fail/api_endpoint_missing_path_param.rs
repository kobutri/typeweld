use typeweld::{api, Json, NoContent};

#[api(get, "/users/{id}")]
pub async fn get_user() -> NoContent {
    unimplemented!()
}

fn main() {}
