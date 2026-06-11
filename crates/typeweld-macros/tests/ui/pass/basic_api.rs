use serde::{Deserialize, Serialize};
use typeweld::{api, api_router, Api, ApiError, ApiRouter, Body, Created, Json, Path, Query};

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: i32,
    pub display_name: String,
    #[serde(default)]
    pub bio: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
pub struct UserId(pub i32);

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(tag = "_tag")]
pub enum Role {
    Admin,
    Member { team: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(rename_all = "camelCase")]
pub struct Filter {
    pub query: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ApiError)]
#[serde(tag = "_tag")]
pub enum UserError {
    #[status(404)]
    NotFound { id: i32 },
    #[status(409)]
    Conflict,
}

#[api(get, "/users/{id}")]
pub async fn get_user(id: Path<i32>) -> Result<Json<User>, UserError> {
    unimplemented!()
}

#[api(get, "/users")]
pub async fn list_users(filter: Query<Filter>) -> Json<Vec<User>> {
    unimplemented!()
}

#[api(post, "/users", allow_unused)]
pub async fn create_user(body: Body<User>) -> Result<Created<User>, UserError> {
    unimplemented!()
}

#[api(delete, "/users/{id}")]
pub async fn delete_user(id: Path<i32>) -> typeweld::NoContent {
    unimplemented!()
}

#[api_router]
pub fn routes() -> ApiRouter {
    ApiRouter::new()
        .endpoint(get_user)
        .endpoint(list_users)
        .endpoint(create_user)
        .endpoint(delete_user)
}

fn main() {}
