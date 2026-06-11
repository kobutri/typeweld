use serde::{Deserialize, Serialize};
use typeweld::{api, Api, Json};

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
pub struct User {
    pub id: i32,
}

#[api(get, "/users")]
pub async fn list_users() -> Result<Json<Vec<User>>, String> {
    unimplemented!()
}

fn main() {}
