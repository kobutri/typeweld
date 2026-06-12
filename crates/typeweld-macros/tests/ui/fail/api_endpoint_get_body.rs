use serde::{Deserialize, Serialize};
use typeweld::{api, Api, Body, Json, NoContent};

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
pub struct Filter {
    pub query: String,
}

#[api(get, "/users")]
pub async fn list_users(filter: Body<Filter>) -> NoContent {
    unimplemented!()
}

fn main() {}
