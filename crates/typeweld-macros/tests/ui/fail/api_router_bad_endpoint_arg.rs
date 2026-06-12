use typeweld::{api, api_router, ApiRouter, NoContent};

#[api(get, "/ping")]
pub async fn ping() -> NoContent {
    unimplemented!()
}

#[api_router]
pub fn routes() -> ApiRouter {
    ApiRouter::new().endpoint(|| ping)
}

fn main() {}
