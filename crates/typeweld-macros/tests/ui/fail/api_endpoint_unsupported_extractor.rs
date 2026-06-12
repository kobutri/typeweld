use typeweld::{api, NoContent};

#[api(get, "/users")]
pub async fn list_users(raw: String) -> NoContent {
    unimplemented!()
}

fn main() {}
