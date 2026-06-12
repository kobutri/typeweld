//! Basic typeweld example: unary endpoints, SSE, and router composition.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::unused_async
)]

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;

use futures_util::stream;
use serde::{Deserialize, Serialize};
use typeweld::{api, api_router, Api, ApiError, ApiRouter, Body, Created, Json, Path, Query, Sse};

/// A user of the system.
#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: i32,
    /// Public display name.
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
#[must_use]
pub fn watch_users() -> Sse<UserEvent, UserEventStream> {
    Sse::new(stream::iter(vec![
        Ok(UserEvent::Created { id: 1 }),
        Ok(UserEvent::Renamed {
            id: 1,
            display_name: "Ada".to_owned(),
        }),
    ]))
}

mod admin {
    use typeweld::{api, NoContent};

    /// Clear all users.
    #[api(delete, "/users")]
    pub async fn clear_users() -> NoContent {
        super::USERS.lock().expect("lock").clear();
        NoContent
    }
}

#[api_router]
#[must_use]
pub fn admin_routes() -> ApiRouter {
    ApiRouter::new().endpoint(admin::clear_users)
}

#[api_router]
#[must_use]
pub fn routes() -> ApiRouter {
    ApiRouter::new()
        .endpoint(get_user)
        .endpoint(list_users)
        .endpoint(create_user)
        .endpoint(watch_users)
        .nest("/admin", admin_routes())
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body as AxumBody};
    use axum::extract::Request;
    use axum::http::{Method, StatusCode};
    use tower::ServiceExt as _;

    use super::*;

    /// The handlers share process-global state (`USERS`); tests that touch
    /// it serialize here so `clear_users` cannot race the round trip.
    static STATE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn create_then_get_round_trip() {
        let _state = STATE_LOCK.lock().await;
        let app = routes().into_router();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/users")
                    .header("content-type", "application/json")
                    .body(AxumBody::from(r#"{"displayName":"Grace"}"#))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let created: serde_json::Value = serde_json::from_slice(&body).expect("json");
        let id = created["id"].as_i64().expect("id");

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/users/{id}"))
                    .body(AxumBody::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn nested_admin_route_serves() {
        let _state = STATE_LOCK.lock().await;
        let app = routes().into_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/admin/users")
                    .body(AxumBody::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn domain_error_carries_status_and_tag() {
        let app = routes().into_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/users/99999")
                    .body(AxumBody::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let error: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(error["_tag"], "UserNotFound");
    }
}
