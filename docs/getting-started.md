# Getting started

## New project

```sh
typeweld new my-app
cd my-app
typeweld generate        # extract + generate the Effect client
cargo run -p server      # start the API server
cd app && npm install && npm run typecheck
```

`typeweld new` scaffolds a cargo workspace with a `server` crate, a
TypeScript `app` wired to the generated bindings via tsconfig `paths`, and a
`typeweld.toml`.

## Existing project

1. Add the facade crate: `cargo add typeweld` (features: `uuid`, `chrono`,
   `decimal`, `json` enable those external types in APIs).
2. Create `typeweld.toml` next to your workspace `Cargo.toml`:

```toml
[[package]]
cargo = "server"                  # cargo package containing the API
ts = "@workspace/server-api"      # generated TypeScript package name

[app]
src = ["app/src/**/*.ts"]         # scanned for usage lints + references

[lint]
unused-endpoints = "warn"         # off | warn | deny
```

3. Annotate your API (see below), run `typeweld generate`, and point your
   app's tsconfig `paths` at `target/typeweld/packages/<name>/index.ts`.

## Declaring an API

**Types** derive `Api` and use plain serde. The supported serde subset is
deliberately restricted to attributes whose wire effect is statically
analyzable: `rename`, `rename_all`, `rename_all_fields`, `tag = "_tag"`
(enums must be internally tagged), `default`,
`skip_serializing_if = "Option::is_none"`, `transparent`,
`deny_unknown_fields`. `flatten`, `untagged`, `skip`, and custom
serializers are compile errors.

**Errors** derive `ApiError` with a `#[status(...)]` per variant. Each
variant becomes a TypeScript `TaggedErrorClass` catchable with
`Effect.catchTag("VariantTag", ...)`.

**Endpoints** are Axum handlers annotated with `#[api(method, "/path")]`
(methods: `get`, `post`, `put`, `patch`, `delete`, `sse`). Parameters use the
typeweld extractors:

- `Path<T>` — path parameters (must match `{name}` segments in the route)
- `Query<T>` — at most one; `T` is a struct whose fields become the query
  parameters (matching Axum's semantics)
- `Body<T>` — JSON body (`post`/`put`/`patch` only)
- `Binary<Bytes>` — raw octet-stream body

Returns: `Json<T>`, `Created<T>`, `NoContent`, `Sse<T, S>`,
`Binary<Bytes>`, or `Result` of those with an `ApiError` enum.

**Routers**: `#[api_router]` functions build an `ApiRouter`, mounting
handlers with `.endpoint(handler)`. `.nest("/prefix", other_routes())` and
`.merge(...)` compose routers and are reflected in generated client routes.
All other Axum composition (`.layer`, `.route_layer`, `.with_state`,
`.fallback`, raw `.route`) passes straight through.

## The generated client

One package per `[[package]]` entry:

```
target/typeweld/packages/@workspace/server-api/
  schemas.ts            # Schema consts + types
  errors.ts             # error classes + unions + decoders
  client.ts             # ApiConfig service + layer(config)
  endpoints/<module>.ts # one accessor per endpoint
  index.ts
```

Accessors return `Effect.Effect<Success, DomainError | ApiClientError,
ApiConfig>` (or `Stream.Stream` for SSE). Provide configuration once with
`Effect.provide(layer({ baseUrl, headers?, timeoutMs?, fetch? }))`. Bundles
stay small: importing one endpoint pulls in only its schemas and the runtime —
there is no API-wide namespace or service object.

## Integer precision

64-bit integers (`i64`, `u64`, `usize`, ...) map to TypeScript `number` and
extraction warns about them: JSON numbers lose precision beyond 2^53. Use
`i32`/`u32` or a string-typed newtype for identifiers.
