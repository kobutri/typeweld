# typeweld

Rust-to-TypeScript API tooling for [Effect](https://effect.website) clients.

Annotate an Axum API with a handful of macros; typeweld statically extracts
the contract from your source code — without compiling anything — and
generates a fully typed, tree-shakeable Effect client package. A language
server welds the two sides together: goto-definition from TypeScript lands in
Rust, find-references on a Rust handler shows its TypeScript call sites, and
renames propagate across the boundary in both directions.

```rust
#[derive(Serialize, Deserialize, Api)]
#[serde(rename_all = "camelCase")]
pub struct User { pub id: i32, pub display_name: String }

#[derive(Serialize, Deserialize, ApiError)]
#[serde(tag = "_tag")]
pub enum UserError {
    #[status(404)]
    UserNotFound { id: i32 },
}

#[api(get, "/users/{id}")]
pub async fn get_user(id: Path<i32>) -> Result<Json<User>, UserError> { ... }

#[api_router]
pub fn routes() -> ApiRouter {
    ApiRouter::new().endpoint(get_user)
}
```

```ts
import { Effect } from "effect"
import { getUser } from "@workspace/api/endpoints/users"
import { layer } from "@workspace/api"

const user = await Effect.runPromise(
  getUser({ id: 1 }).pipe(
    Effect.catchTag("UserNotFound", () => Effect.succeed(null)),
    Effect.provide(layer({ baseUrl: "http://127.0.0.1:3000" })),
  ),
)
```

## Pieces

| Piece | What it does |
| --- | --- |
| `typeweld` (crate) | Macros + Axum runtime: `#[api]`, `#[api_router]`, `derive(Api)`, `derive(ApiError)` |
| `typeweld` (npm / binary) | CLI: `new`, `generate [--watch]`, `check [--unused]`, `lsp` |
| `@typeweld/effect-runtime` | The small runtime the generated client calls into |
| `typeweld-ls` | Language server: cross-language navigation, rename, live regeneration |
| VS Code extension | Wires the language server up automatically |

## Documentation

- [Getting started](getting-started.md)
- [Architecture](architecture.md)
- [Editor setup](editors.md)

## Properties worth knowing

- **Generation never compiles your crate.** The contract is extracted by
  parsing source (syn), so `typeweld generate` is milliseconds — fast enough
  to run on every keystroke in the language server.
- **What compiles is what extracts.** All validation rules are shared between
  the proc macros and the extractor; cross-file types are guarded by
  `ApiBound` trait assertions emitted by the derives.
- **Generated bindings are transient.** They land in
  `target/typeweld/packages/` and are never committed.
- **Wire-accurate types.** The generated Effect schemas mirror serde's actual
  wire format (a restricted, statically analyzable serde subset is enforced).
