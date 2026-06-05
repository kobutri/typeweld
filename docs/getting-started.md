# Getting Started

This project turns a Rust API contract into an Effect-native TypeScript package.
Rust remains the source of truth: endpoint routes, request shapes, response
types, domain errors, and symbol locations all come from Rust metadata.

## 1. Define Rust API types

```rust
use api_core::{Json, Path};
use api_macros::{api, ApiError, ApiType};
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
```

Export endpoints through an explicit module so collection only includes routes
you chose to publish:

```rust
use api_core::api_module;

pub fn api() -> api_core::ApiModule {
    api_module!(name = "users", endpoints = [get_user])
}
```

## 2. Collect the contract

```sh
cargo run -p api-collector --bin api -- collect \
  --package-name @workspace/server-api \
  --out target/api-contract/server-api.json
```

## 3. Generate the hidden TypeScript package

```sh
cargo run -p api-collector --bin api -- gen \
  --contract target/api-contract/server-api.json \
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

Use `api doctor` when setup is missing or a workspace cannot be discovered.
