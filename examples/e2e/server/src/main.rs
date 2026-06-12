//! End-to-end example server: unary, SSE, and binary endpoints.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;

use futures_util::stream;
use serde::{Deserialize, Serialize};
use typeweld::{
    api, api_router, Api, ApiError, ApiRouter, Binary, Body, Bytes, Created, Json, Path, Query,
    Sse,
};

/// A user of the system.
#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: i32,
    pub display_name: String,
    #[serde(default)]
    pub bio: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(rename_all = "camelCase")]
pub struct CreateUser {
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(rename_all = "camelCase")]
pub struct UserFilter {
    pub query: Option<String>,
    pub limit: u32,
}

/// An event about a user.
#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(tag = "_tag")]
pub enum UserEvent {
    Created { id: i32 },
    Renamed { id: i32, display_name: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, ApiError)]
#[serde(tag = "_tag", rename_all = "PascalCase")]
pub enum UserError {
    /// No user with this id exists.
    #[status(404)]
    UserNotFound { id: i32 },
    #[status(409)]
    DisplayNameTaken { display_name: String },
}

static NEXT_ID: AtomicI32 = AtomicI32::new(1);
static USERS: Mutex<Vec<User>> = Mutex::new(Vec::new());
static AVATARS: Mutex<Vec<(i32, Vec<u8>)>> = Mutex::new(Vec::new());

/// Fetch one user by id.
#[api(get, "/users/{id}")]
pub async fn get_user(id: Path<i32>) -> Result<Json<User>, UserError> {
    let users = USERS.lock().expect("lock");
    users
        .iter()
        .find(|user| user.id == *id)
        .cloned()
        .map(Json)
        .ok_or(UserError::UserNotFound { id: *id })
}

/// List users matching a filter.
#[api(get, "/users")]
pub async fn list_users(filter: Query<UserFilter>) -> Json<Vec<User>> {
    let users = USERS.lock().expect("lock");
    let matching = users
        .iter()
        .filter(|user| {
            filter
                .query
                .as_deref()
                .is_none_or(|query| user.display_name.contains(query))
        })
        .take(filter.limit as usize)
        .cloned()
        .collect();
    Json(matching)
}

/// Create a new user.
#[api(post, "/users")]
pub async fn create_user(body: Body<CreateUser>) -> Result<Created<User>, UserError> {
    let mut users = USERS.lock().expect("lock");
    if users
        .iter()
        .any(|user| user.display_name == body.display_name)
    {
        return Err(UserError::DisplayNameTaken {
            display_name: body.display_name.clone(),
        });
    }
    let user = User {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        display_name: body.display_name.clone(),
        bio: None,
    };
    users.push(user.clone());
    Ok(Created(user))
}

type UserEventStream = stream::Iter<std::vec::IntoIter<Result<UserEvent, UserError>>>;

/// Watch user events as server-sent events.
#[api(sse, "/events/users")]
pub fn watch_users() -> Sse<UserEvent, UserEventStream> {
    Sse::new(stream::iter(vec![
        Ok(UserEvent::Created { id: 1 }),
        Ok(UserEvent::Renamed {
            id: 1,
            display_name: "Ada".to_owned(),
        }),
    ]))
}

/// Upload a user avatar.
#[api(post, "/avatars/{id}")]
pub async fn upload_avatar(id: Path<i32>, data: Binary<Bytes>) -> Result<Json<User>, UserError> {
    let users = USERS.lock().expect("lock");
    let user = users
        .iter()
        .find(|user| user.id == *id)
        .cloned()
        .ok_or(UserError::UserNotFound { id: *id })?;
    AVATARS.lock().expect("lock").push((*id, data.to_vec()));
    Ok(Json(user))
}

/// Download a user avatar.
#[api(get, "/avatars/{id}")]
pub async fn download_avatar(id: Path<i32>) -> Result<Binary<Bytes>, UserError> {
    let avatars = AVATARS.lock().expect("lock");
    avatars
        .iter()
        .find(|(user_id, _)| user_id == &*id)
        .map(|(_, data)| Binary(Bytes::from(data.clone())))
        .ok_or(UserError::UserNotFound { id: *id })
}

#[api_router]
pub fn routes() -> ApiRouter {
    ApiRouter::new()
        .endpoint(get_user)
        .endpoint(list_users)
        .endpoint(create_user)
        .endpoint(watch_users)
        .endpoint(upload_avatar)
        .endpoint(download_avatar)
}

#[tokio::main]
async fn main() {
    let address = std::env::var("E2E_ADDR").unwrap_or_else(|_| "127.0.0.1:39123".to_owned());
    let listener = tokio::net::TcpListener::bind(&address).await.expect("bind");
    println!("listening on http://{address}");
    axum::serve(listener, routes().into_router())
        .await
        .expect("serve");
}
