use serde::{Deserialize, Serialize};
use typeweld::{api, Api, NoContent, Query};

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
pub struct Filter {
    pub query: String,
}

#[api(get, "/users")]
pub async fn list_users(a: Query<Filter>, b: Query<Filter>) -> NoContent {
    unimplemented!()
}

fn main() {}
