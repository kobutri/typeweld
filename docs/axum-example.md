# Axum Example Guide

The Axum adapter converts explicit API metadata into Axum routes while keeping
the contract layer framework-neutral.

## Runnable shape

```rust
use api_axum::{router, Json, Path};
use api_core::api_module;
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
    let module = api_module!(name = "users", endpoints = [get_user]);

    router(module)
        .route(__api_endpoint_get_user(), |path: Path<i64>| async move {
            api_axum::success_or_error(get_user(path).await)
        })
        .into_router()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    println!("listening on http://{}", listener.local_addr()?);

    axum::serve(listener, app()).await?;
    Ok(())
}
```

The `app()` function only builds the Axum router. A binary still needs to bind a
listener and pass that router to `axum::serve`.

## Server-sent events

Use the `api_axum::Sse<T, Stream>` wrapper for streaming Axum handlers. The
`#[api]` macro records the same stream item shape in framework-neutral metadata,
and the generated TypeScript accessor returns
`Stream.Stream<Item, Error, ServerApi>`.

See `examples/axum-sse` for the current SSE shape.

## Notes

- Endpoints are registered only when included in an `api_module!`.
- Domain errors should derive `ApiError` and declare HTTP status metadata.
- The generated TypeScript success type is the decoded response body.
- In runnable Axum handlers, import `Json`, `Path`, `Query`, `Body`, `Created`,
  `NoContent`, and `Sse` from `api_axum`; the `api_core` wrappers are
  framework-neutral markers for contract-only code.
