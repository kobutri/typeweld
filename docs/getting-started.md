# Getting Started

This project turns a Rust API contract into an Effect-native TypeScript package.
Rust remains the source of truth: endpoint routes, request shapes, response
types, domain errors, and symbol locations all come from Rust metadata.

## 0. Install repository TypeScript tooling

The repository's JavaScript workspace lives under `npm/` and is locked with
`npm/package-lock.json`:

```sh
npm --prefix npm ci
```

Run the same TypeScript validation command used by CI with:

```sh
npm --prefix npm test
```

That command typechecks the runtime and language-server wrapper workspaces,
typechecks the generated-package fixture against the pinned Effect beta, and
runs the runtime and wrapper tests. For narrower checks, use
`npm --prefix npm run typecheck:generated`, `npm --prefix npm run test:runtime`,
or `npm --prefix npm run test:lsp-wrapper`.

## 1. Define Rust API types

```rust
use api_axum::{ApiRouter, Json, Path};
use api_macros::{api, api_router, ApiError, ApiType};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, ApiType)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: i64,
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ApiError)]
#[serde(tag = "_tag")]
pub enum GetUserError {
    #[api_error(status = 404)]
    UserNotFound { id: i64 },
}

#[api(method = "GET", path = "/users/{id}")]
pub async fn get_user(Path(id): Path<i64>) -> Result<Json<User>, GetUserError> {
    todo!("load user {id}")
}

#[api_router]
pub fn routes() -> ApiRouter {
    ApiRouter::new("users").endpoint(get_user)
}
```

The `ApiRouter` is the export boundary. Only endpoints mounted with
`.endpoint(handler)` are collected and generated for TypeScript. In runnable
Axum handlers, import `Json`, `Path`, `Query`, `Body`, `Created`, `NoContent`,
and `Sse` from `api_axum`; the `api_core` wrappers remain available for
framework-neutral contract-only code.

## 2. Collect the contract

Declare the TypeScript package name and Rust root function in the Rust package
metadata:

```toml
[package.metadata.rust_ts]
ts_package = "@workspace/server-api"
api_root = "server::routes"
features = []
```

```sh
cargo run -p api-collector --bin api -- collect \
  --package server \
  --out target/api-contract/server-api.json
```

Legacy roots built with `api_module!` still compile and can remain on
`api_root = "server::api"` during migration. New Axum code should prefer
`#[api_router]` plus `.endpoint(get_user)` so the runtime router and exported
contract stay in one tree.

## 3. Generate the hidden TypeScript package

```sh
cargo run -p api-collector --bin api -- gen \
  --contract target/api-contract/server-api.json \
  --target-dir target
```

During local development, use `api watch` to keep the hidden package and symbol
graph refreshed as Rust source changes:

```sh
cargo run -p api-collector --bin api -- watch \
  --package server \
  --target-dir target
```

The package is written below
`target/api-contract/effect-v4/packages/_workspace_server-api`. Keep it hidden
and generated. Do not commit it.

## 4. Point TypeScript at the package

Add the generated paths file to your TypeScript config flow, or copy the same
`compilerOptions.paths` entries into your workspace `tsconfig.json`.

```json
{
  "extends": "./target/api-contract/effect-v4/packages/_workspace_server-api/tsconfig.paths.json"
}
```

## 5. Call the endpoint from Effect

```ts
import { Effect } from "effect"
import { ServerApi, users } from "@workspace/server-api"

const program = Effect.gen(function* () {
  const user = yield* users.getUser({ id: 1 })
  return user.displayName
}).pipe(Effect.provide(ServerApi.layer({ baseUrl: "http://localhost:3000" })))
```

Rust `Result<T, E>` maps to `Effect.Effect<T, E, R>`. Domain errors are in the
Effect error channel, not thrown promises and not a success-channel `Result`.

## 6. Check generated state and usages

```sh
cargo run -p api-collector --bin api -- check \
  --contract target/api-contract/server-api.json \
  --target-dir target

cargo run -p api-collector --bin api -- check-usages \
  --contract target/api-contract/server-api.json \
  --out target/api-contract/graph/effect-usage-index.json \
  --ts-dir app/src
```

Use `api doctor` when setup is missing or a workspace cannot be discovered. If
you use `api-ls`, configure `apiWatch` in `.api-ls.json` so the editor starts
and owns the same watcher for you.
