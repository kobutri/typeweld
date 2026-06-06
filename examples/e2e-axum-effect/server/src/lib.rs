use std::convert::Infallible;

use api_axum::{router, Body, Created, Json, Path, Sse};
use api_collector::{collect_contract, CollectorInput};
use api_core::{api_module, ir::ApiContract, ApiModule, ApiType};
use axum::{response::Response, Router};
use futures_util::stream;
use serde::{Deserialize, Serialize};

pub const TS_PACKAGE_NAME: &str = "@workspace/e2e-api";

#[derive(Clone, Debug, Deserialize, Serialize, api_macros::ApiType)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: i32,
    pub display_name: String,
    pub role: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, api_macros::ApiType)]
#[serde(rename_all = "camelCase")]
pub struct CreateUserRequest {
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, api_macros::ApiType)]
#[serde(rename_all = "camelCase")]
pub struct UserEvent {
    pub id: i32,
    pub kind: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, api_macros::ApiType)]
#[serde(rename_all = "camelCase")]
pub struct AuditLog {
    pub summary: String,
    pub generated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, api_macros::ApiError)]
#[serde(tag = "_tag", rename_all = "PascalCase")]
pub enum UserError {
    #[api_error(status = 404)]
    UserNotFound { id: i32 },
    #[api_error(status = 409)]
    DisplayNameTaken { display_name: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, api_macros::ApiError)]
#[serde(tag = "_tag", rename_all = "PascalCase")]
pub enum EventError {
    #[api_error(status = 401)]
    Unauthorized,
}

type UserEventStream = stream::Iter<std::array::IntoIter<Result<UserEvent, EventError>, 2>>;

/// Loads the sample user.
///
/// # Errors
///
/// Returns [`UserError::UserNotFound`] when the requested id is not available.
#[api_macros::api(method = "GET", path = "/users/{id}")]
pub async fn get_user(id: Path<i32>) -> Result<Json<User>, UserError> {
    let id = id.into_inner();
    if id == 1 {
        Ok(Json(user(1, "Ada Lovelace", "admin")))
    } else {
        Err(UserError::UserNotFound { id })
    }
}

/// Creates a sample user.
///
/// # Errors
///
/// Returns [`UserError::DisplayNameTaken`] for the reserved display name used by
/// the runtime error-channel check.
#[api_macros::api(method = "POST", path = "/users")]
pub async fn create_user(request: Body<CreateUserRequest>) -> Result<Created<User>, UserError> {
    let request = request.into_inner();
    if request.display_name == "Root" {
        Err(UserError::DisplayNameTaken {
            display_name: request.display_name,
        })
    } else {
        Ok(Created(user(2, &request.display_name, "member")))
    }
}

/// Streams user events.
///
/// # Errors
///
/// The example stream type includes [`EventError`] so generated SSE error
/// channels are represented. The current sample data only emits successful
/// events.
#[api_macros::api(method = "SSE", path = "/events/users")]
pub fn watch_users() -> Result<Sse<UserEvent, UserEventStream>, EventError> {
    Ok(Sse::new(stream::iter([
        Ok(UserEvent {
            id: 1,
            kind: "connected".to_owned(),
            display_name: "Ada Lovelace".to_owned(),
        }),
        Ok(UserEvent {
            id: 2,
            kind: "created".to_owned(),
            display_name: "Grace Hopper".to_owned(),
        }),
    ])))
}

#[api_macros::api(method = "GET", path = "/admin/audit-log")]
pub async fn audit_log() -> Json<AuditLog> {
    Json(AuditLog {
        summary: "4 endpoint(s) checked".to_owned(),
        generated_at: "2026-06-06T00:00:00Z".to_owned(),
    })
}

#[must_use]
pub fn api() -> ApiModule {
    api_module!(
        name = "e2e",
        endpoints = [get_user, create_user, watch_users, audit_log]
    )
}

#[must_use]
pub fn contract() -> ApiContract {
    collect_contract(CollectorInput {
        package_name: TS_PACKAGE_NAME.to_owned(),
        root_module: api(),
        types: Vec::new(),
        errors: Vec::new(),
    })
}

pub fn app() -> Router {
    router(api())
        .route(__api_endpoint_get_user(), get_user_response)
        .route(__api_endpoint_create_user(), create_user_response)
        .route(__api_endpoint_watch_users(), watch_users_response)
        .route(__api_endpoint_audit_log(), audit_log_response)
        .into_router()
}

async fn get_user_response(id: Path<i32>) -> Response {
    api_axum::success_or_error(get_user(id).await)
}

async fn create_user_response(request: Body<CreateUserRequest>) -> Response {
    api_axum::success_or_error(create_user(request).await)
}

async fn watch_users_response() -> Response {
    api_axum::success_or_error(watch_users())
}

async fn audit_log_response() -> Result<Json<AuditLog>, Infallible> {
    Ok(audit_log().await)
}

fn user(id: i32, display_name: &str, role: &str) -> User {
    User {
        id,
        display_name: display_name.to_owned(),
        role: role.to_owned(),
    }
}
