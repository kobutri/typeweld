//! A heavily commented Rust half of a tiny Rust + TypeScript monorepo.
//!
//! Run it with:
//!
//! ```sh
//! cargo run -p monorepo-basic
//! ```
//!
//! The binary prints the API contract that a real workspace would write to
//! `target/api-contract/server-api.json`. The neighboring `app/` directory
//! shows the TypeScript half that imports the generated package.

use api_axum::{ApiRouter, Body, Json, Path};
use api_collector::{collect_contract, contract_to_json, CollectorInput};
use serde::{Deserialize, Serialize};

/// This is a DTO: a data type that crosses the Rust/TypeScript boundary.
///
/// `ApiType` produces the framework-neutral schema metadata used by the
/// generator. `Serialize` and `Deserialize` are still the real wire contract:
/// Serde decides what JSON looks like on the network.
#[derive(Clone, Debug, Deserialize, Serialize, api_macros::ApiType)]
#[serde(rename_all = "camelCase")]
struct User {
    /// Rust code uses snake_case.
    id: i64,

    /// TypeScript and JSON use `displayName` because of `rename_all`.
    display_name: String,
}

/// Request DTO for `POST /users`.
///
/// Keeping request and response bodies as named DTOs makes generated TypeScript
/// easier to read and gives the LSP stable symbols to link back to Rust.
#[derive(Clone, Debug, Deserialize, Serialize, api_macros::ApiType)]
#[serde(rename_all = "camelCase")]
struct CreateUserRequest {
    display_name: String,
}

/// Public endpoint errors must be typed and declared.
///
/// The generated Effect client puts these variants in the Effect error channel,
/// alongside generated client/transport errors.
#[derive(Clone, Debug, Deserialize, Serialize, api_macros::ApiError)]
#[serde(tag = "_tag", rename_all = "PascalCase")]
enum UserError {
    /// The status code is part of the API contract.
    #[api_error(status = 404)]
    UserNotFound { id: i64 },

    /// This variant demonstrates a non-404 domain error.
    #[api_error(status = 409)]
    DisplayNameTaken { display_name: String },
}

/// `#[api]` is the contract boundary.
///
/// This function can be implemented however your server wants, but its public
/// signature is what the collector and generator care about:
///
/// - `GET` and `/users/{id}` become route metadata.
/// - `Path<i64>` becomes a generated `id` argument.
/// - `Json<User>` becomes the Effect success type.
/// - `UserError` becomes part of the Effect error type.
#[api_macros::api(method = "GET", path = "/users/{id}")]
#[allow(dead_code)]
async fn get_user(id: Path<i64>) -> Result<Json<User>, UserError> {
    let Path(id) = id;

    if id == 1 {
        Ok(Json(User {
            id,
            display_name: "Ada Lovelace".to_owned(),
        }))
    } else {
        Err(UserError::UserNotFound { id })
    }
}

/// Body extractors become a generated `body` argument in TypeScript.
///
/// In a real Axum application an adapter would decode the HTTP body before this
/// handler is called. This example keeps the function plain so the contract is
/// easy to see.
#[api_macros::api(method = "POST", path = "/users")]
#[allow(dead_code)]
async fn create_user(request: Body<CreateUserRequest>) -> Result<Json<User>, UserError> {
    let Body(request) = request;

    if request.display_name == "Root" {
        Err(UserError::DisplayNameTaken {
            display_name: request.display_name,
        })
    } else {
        Ok(Json(User {
            id: 2,
            display_name: request.display_name,
        }))
    }
}

/// The root router is intentionally explicit.
///
/// Only endpoints listed here are exported to TypeScript and tracked by unused
/// endpoint analysis. This avoids surprising "everything public is an API"
/// behavior in larger workspaces.
#[api_macros::api_router]
fn routes() -> ApiRouter {
    ApiRouter::new("server")
        .endpoint(get_user)
        .endpoint(create_user)
}

fn main() {
    // In an application, collection is usually driven by build tooling.
    //
    // This example performs the same core operation in-process so you can run
    // one binary and inspect the resulting contract.
    let contract = collect_contract(CollectorInput::from_root("@workspace/server-api", routes()));

    eprintln!("Collected {} endpoint(s)", contract.endpoints.len());
    println!(
        "{}",
        contract_to_json(&contract).expect("example contract should serialize")
    );
}
