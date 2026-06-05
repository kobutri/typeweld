# Axum Example Guide

The Axum adapter converts explicit API metadata into Axum routes while keeping
the contract layer framework-neutral.

## Basic shape

```rust
use api_axum::{method_router, router};
use api_core::{api_module, ir::HttpMethod, Json, Path};
use api_macros::{api, ApiError, ApiType};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, ApiType)]
#[serde(rename_all = "camelCase")]
struct User {
    id: i64,
    display_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ApiError)]
#[serde(tag = "_tag")]
enum GetUserError {
    #[api_error(status = 404)]
    UserNotFound { id: i64 },
}

#[api(method = "GET", path = "/users/{id}")]
async fn get_user(Path(id): Path<i64>) -> Result<Json<User>, GetUserError> {
    Ok(Json(User {
        id,
        display_name: "Ada".to_owned(),
    }))
}

fn app() -> axum::Router {
    let module = api_module!(name = "users", endpoints = [__api_endpoint_get_user]);

    router(module)
        .route(
            __api_endpoint_get_user(),
            method_router(HttpMethod::Get, || async {
                api_axum::success_or_error(get_user(Path(1)).await)
            }),
        )
        .into_router()
}
```

## Server-sent events

Use the `Sse<T, Stream>` wrapper for streaming endpoint metadata. The generated
TypeScript accessor returns `Stream.Stream<Item, Error, ServerApi>`.

See `examples/axum-sse` for the current SSE shape.

## Notes

- Endpoints are registered only when included in an `api_module!`.
- Domain errors should derive `ApiError` and declare HTTP status metadata.
- The generated TypeScript success type is the decoded response body.
