//! Rust-to-TypeScript API contract tooling for Effect clients.
//!
//! Declare your Axum API with a handful of attributes and derive macros; the
//! `typeweld` CLI and language server statically extract the contract and
//! generate a typed Effect client — without ever compiling your crate.
//!
//! ```ignore
//! use serde::{Deserialize, Serialize};
//! use typeweld::{api, api_router, Api, ApiError, ApiRouter, Json, Path};
//!
//! #[derive(Serialize, Deserialize, Api)]
//! #[serde(rename_all = "camelCase")]
//! pub struct User { pub id: i32, pub display_name: String }
//!
//! #[derive(Serialize, Deserialize, ApiError)]
//! #[serde(tag = "_tag")]
//! pub enum UserError {
//!     #[status(404)]
//!     NotFound { id: i32 },
//! }
//!
//! #[api(get, "/users/{id}")]
//! pub async fn get_user(id: Path<i32>) -> Result<Json<User>, UserError> {
//!     # unimplemented!()
//! }
//!
//! #[api_router]
//! pub fn routes() -> ApiRouter {
//!     ApiRouter::new().endpoint(get_user)
//! }
//! ```

pub use typeweld_axum::{
    ApiBound, ApiRouter, ApiStatus, Binary, Body, Bytes, Created, Json, NoContent, Path, Query,
    Response, Sse,
};
pub use typeweld_macros::{api, api_router, Api, ApiError};

/// Runtime items referenced by macro-generated code. Not public API.
#[doc(hidden)]
pub mod __rt {
    pub use typeweld_axum::{
        into_api_response, method_router, success_or_error_response, ApiBound, ApiStatus,
        EndpointMount, HttpMethod, Response,
    };
}
