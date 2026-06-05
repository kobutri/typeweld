# Rust ↔ TypeScript Effect API Integration — Implementation Plan

This document describes the implementation plan for a greenfield tool that turns Rust API endpoints into an Effect-native TypeScript API layer, while providing bidirectional cross-language editor features through a single LSP gateway.

The tool is not just a Rust-to-TypeScript type emitter. It is a Rust API contract compiler, TypeScript/Effect generator, usage analyzer, and cross-language language server.

## 1. Locked decisions

The following decisions are treated as requirements for the implementation plan.

### 1.1 Source of truth

Rust is the source of truth.

The hand-authored Rust API surface defines:

- endpoints;
- request inputs;
- success outputs;
- typed domain errors;
- streaming transports;
- route paths and path parameters;
- Serde wire shape;
- cross-language symbol identity.

TypeScript artifacts are generated on the fly and are not checked in.

### 1.2 TypeScript paradigm

The TypeScript side is Effect v4 native.

Generated endpoint functions return:

```ts
Effect.Effect<Success, Error, Requirements>
```

Generated server streams return:

```ts
Stream.Stream<Item, Error, Requirements>
```

Rust `Result<T, E>` maps to the Effect error channel, not to a success-channel `Result` and not to thrown promise errors.

### 1.3 LSP architecture

Use gateway mode only.

There is no companion-mode fallback. `api-ls` is the language-server entry point for workspaces using this tool. It launches and proxies rust-analyzer and the TypeScript/Effect language server internally, then merges, rewrites, and augments results.

### 1.4 Framework strategy

Use a framework-neutral contract core, with Axum as the first server framework adapter.

The contract layer should not be tightly coupled to Axum, Actix, Poem, Rocket, Tonic, or a custom HTTP stack. The MVP ships Axum integration because it is a practical first target.

### 1.5 API style

Support REST-ish HTTP endpoint annotations first.

The initial authoring model is:

```rust
#[api(GET, "/users/{id}")]
pub async fn get_user(...) -> Result<Json<User>, GetUserError> {
    todo!()
}
```

Internal IR should be general enough to later support RPC-style endpoints, generated Effect RPC metadata, and generated Effect HttpApi metadata.

### 1.6 Generated Effect surface

Generate our own stable public Effect service and endpoint accessor surface.

Optionally generate Effect HttpApi and/or Effect RPC metadata underneath, but do not make the Rust authoring model depend directly on Effect's specific TS API shape.

Public TS surface:

```ts
users.getUser(args): Effect.Effect<User, GetUserError | ApiClientError, ServerApi>
```

Generated runtime surface:

```ts
ServerApi.layer(config): Layer.Layer<ServerApi, never, HttpClient | AuthProvider | Tracer>
```

### 1.7 Error model

Require declared, typed API errors.

Allowed:

```rust
#[derive(ApiError, ApiType, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum GetUserError {
    #[api(status = 404)]
    UserNotFound { id: UserId },

    #[api(status = 403)]
    PermissionDenied,
}
```

Rejected in public endpoints unless explicitly mapped before crossing the API boundary:

```rust
anyhow::Error
Box<dyn std::error::Error>
String
std::io::Error
```

### 1.8 Success response model

Support typed response wrappers.

Initial wrappers:

```rust
Json<T>          // JSON 200 by default
Created<T>       // JSON 201
NoContent        // 204
Binary<T>        // binary response
Sse<T>           // server-sent event stream
```

The generated Effect success type is usually the decoded success body, not an HTTP response wrapper. Status and headers become part of the TS success type only when they are explicitly part of the Rust contract.

### 1.9 Wire encoding policy

Serde defines the JSON wire shape.

Defaults:

| Rust type | TS/Effect encoded shape | TS/Effect decoded shape |
|---|---:|---:|
| `String` | `string` | `string` |
| `bool` | `boolean` | `boolean` |
| `i8`, `i16`, `i32`, `u8`, `u16`, `u32` | `number` | `number` |
| `usize`, `isize` | `number` | `number` |
| `i64`, `u64`, `i128`, `u128` | `string` | branded string |
| `uuid::Uuid` | `string` | branded string |
| `chrono::DateTime<Utc>` | `string` | branded ISO string initially |
| `rust_decimal::Decimal` | `string` | branded decimal string initially |
| `Option<T>` | `T | null` | `T | null` |
| `Option<T>` with Serde omit-if-none behavior | optional property | optional property |
| `Vec<T>` | `ReadonlyArray<T>` | `ReadonlyArray<T>` |
| `HashMap<String, V>` | `Record<string, V>` | `Record<string, V>` |
| JSON bytes | base64 string | branded base64 string |
| binary endpoint bytes | `Uint8Array` / `Blob` | `Uint8Array` / `Blob` |
| newtype struct | underlying encoded type | branded decoded type |
| `serde_json::Value` | `JsonValue` | `JsonValue` |

Rich decoded DateTime/Decimal values can be added later behind configuration. The MVP should prefer branded strings to reduce runtime dependency complexity.

### 1.10 Field and wire rename semantics

Field-level rename changes the wire contract.

Example starting point:

```rust
#[derive(ApiType, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub display_name: String,
}
```

Generated TS:

```ts
export interface User {
  readonly displayName: string
}
```

If the user renames `displayName` to `fullName` from TS, the refactor is treated as an API contract rename. The Rust field should become:

```rust
pub full_name: String,
```

or, if the surrounding Serde rules cannot express the exact target name cleanly, the tool should update/add the minimal Serde attributes needed to make the wire contract become `fullName`.

The tool must not silently preserve the old wire name using `#[serde(rename = "displayName")]` unless the user invokes a future explicit compatibility-preserving refactor.

### 1.11 Routing integration

Use explicit API module/router composition.

Do not auto-register every `#[api]` function in a crate. Export endpoints reachable from an explicit root:

```rust
pub fn api() -> ApiModule<AppCtx> {
    api_module! {
        users::get_user,
        users::create_user,
        events::events,
    }
}
```

This avoids surprising registration behavior and gives a clear Rust reference path.

### 1.12 Unused endpoint semantics

An endpoint counts as used only if it has a strong TypeScript/Effect usage.

Strong usage examples:

```ts
yield* users.getUser({ id })
return users.getUser({ id })
users.getUser({ id }).pipe(Effect.retry(...))
Layer.effect(UserService, users.getUser({ id }))
```

Weak or non-usage examples:

```ts
import { users } from "@workspace/server-api"
users.getUser
users.getUser({ id }) // floating Effect, not composed or yielded
```

Unused endpoint warning policy:

```text
Warn when an exported Rust endpoint has no strong TypeScript Effect usages.
```

Escape hatch:

```rust
#[api(GET, "/admin/reindex", allow_unused)]
pub async fn reindex(...) -> Result<Json<ReindexResult>, ReindexError> {
    todo!()
}
```

CI can turn warnings into failures.

### 1.13 Workspace/package strategy

Assume one Cargo workspace and one JS package-manager workspace, with many Rust crates and many TS projects.

Generated TS packages mirror Cargo API crates.

Rust crate metadata:

```toml
[package.metadata.api]
export = true
ts_package = "@workspace/server-api"
```

Generated TypeScript package location:

```text
target/api-contract/effect-v4/packages/server-api/
```

TS import:

```ts
import { users, ServerApi } from "@workspace/server-api"
```

Development resolution uses `tsconfig.paths` pointing to generated cache files. The LSP hides generated files during navigation.

## 2. Goals

### 2.1 User-facing goals

- Write endpoint code once in Rust.
- Generate Effect-native TypeScript APIs on the fly.
- Do not check generated files into git.
- Keep generated files transparent during editor navigation.
- Make TS go-to-definition jump to Rust endpoints/types/fields/error variants.
- Make Rust find-references include TS usages.
- Make TS find-references include Rust endpoints/router registrations where appropriate.
- Support field-level Rust ↔ TS navigation.
- Support bidirectional rename for API symbols.
- Make unsupported API shapes fail Rust compilation.
- Warn when exported Rust endpoints have no strong TS/Effect usages.
- Work in any editor that can speak LSP by using one gateway server.

### 2.2 Engineering goals

- Keep Rust API contract IR independent of TS code generation.
- Keep Effect v4-specific code inside a generator backend.
- Make the LSP symbol graph the single source of truth for cross-language identity.
- Make the generated package usable by non-editor tools such as `tsc` and test runners.
- Preserve exact source ranges for endpoints, fields, errors, and generated TS symbols.
- Allow tasks to be built and tested independently.

## 3. Non-goals for the MVP

The MVP does not need to support:

- WebSocket duplex APIs.
- Multipart form upload.
- gRPC/Tonic.
- OpenAPI generation.
- Zod/Valibot generation.
- Rich decoded DateTime/Decimal objects.
- Compatibility-preserving field rename.
- Runtime server implementation generation for every Rust framework.
- Native rustc-driver lint integration with perfect spans.
- Publishing generated TS packages to npm.

These are explicitly planned as later phases where the IR should already be ready.

## 4. High-level architecture

```text
Rust workspace
  crates/server/src/users.rs
  crates/server/src/events.rs
       │
       │ #[api(...)] endpoints
       │ #[derive(ApiType)] DTOs
       │ #[derive(ApiError)] errors
       ▼
api contract compiler
  - rustc-backed validation
  - Serde lowering
  - endpoint extraction
  - source ranges
  - API IR
       │
       ├─────────────────────────────┐
       ▼                             ▼
Effect v4 generator              symbol graph builder
  - Schema values                 - Rust symbol IDs
  - tagged errors                 - TS symbol IDs
  - Effect service                - field links
  - Layer                         - error links
  - endpoint accessors            - route links
       │                             │
       ▼                             ▼
target/api-contract/...          target/api-contract/graph/...
       │                             │
       └──────────────┬──────────────┘
                      ▼
api-ls gateway
  - starts rust-analyzer
  - starts TypeScript/Effect language service
  - serves generated files to TS backend
  - redirects generated TS definitions to Rust
  - merges references
  - performs cross-language rename
  - emits unused endpoint diagnostics
                      │
                      ▼
Editor over LSP
```

## 5. User-facing Rust API

### 5.1 Basic endpoint

```rust
use api::{api, ApiError, ApiType, Json, Path};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, ApiType)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: UserId,
    pub display_name: String,
}

#[derive(Clone, Serialize, Deserialize, ApiType)]
pub struct UserId(uuid::Uuid);

#[derive(Debug, Serialize, Deserialize, ApiError, ApiType)]
#[serde(tag = "_tag", rename_all = "PascalCase")]
pub enum GetUserError {
    #[api(status = 404)]
    UserNotFound { id: UserId },

    #[api(status = 403)]
    PermissionDenied,
}

#[api(GET, "/users/{id}")]
pub async fn get_user(
    id: Path<UserId>,
) -> Result<Json<User>, GetUserError> {
    todo!()
}
```

Generated TS usage:

```ts
import { Effect } from "effect"
import { ServerApi, users } from "@workspace/server-api"

const program = Effect.gen(function* () {
  const user = yield* users.getUser({ id })
  return user.displayName
}).pipe(
  Effect.catchTag("UserNotFound", () => Effect.succeed(null)),
  Effect.provide(ServerApi.layer({ baseUrl: "/api" }))
)
```

### 5.2 Explicit module composition

```rust
pub fn api() -> ApiModule<AppCtx> {
    api_module! {
        users::get_user,
        users::create_user,
        events::events,
    }
}
```

Only endpoints reachable from the root API module are exported to TS and tracked by unused endpoint analysis.

### 5.3 Server stream

```rust
#[derive(Clone, Serialize, Deserialize, ApiType)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: EventId,
    pub kind: EventKind,
}

#[derive(Debug, Serialize, Deserialize, ApiError, ApiType)]
#[serde(tag = "_tag")]
pub enum EventError {
    Unauthorized,
}

#[api(SSE, "/events")]
pub fn events(
    ctx: Ctx,
) -> impl futures_core::Stream<Item = Result<Event, EventError>> {
    todo!()
}
```

Generated TS:

```ts
export namespace events {
  export const events: () => Stream.Stream<
    Event,
    EventError | ApiClientError,
    ServerApi
  >
}
```

## 6. Generated TypeScript/Effect API

### 6.1 Public package shape

Generated package:

```text
target/api-contract/effect-v4/packages/server-api/
  package.json
  index.ts
  schemas.ts
  errors.ts
  client.ts
  layer.ts
  endpoints.ts
```

Public imports:

```ts
import { users, events, ServerApi } from "@workspace/server-api"
```

### 6.2 Generated schemas

For every exported API type, generate an Effect Schema value and decoded/encoded type aliases.

Conceptual generated shape:

```ts
export const UserId = Schema.String.pipe(brand("server::users::UserId"))
export type UserId = typeof UserId.Type
export type UserIdEncoded = typeof UserId.Encoded

export const User = Schema.Struct({
  id: UserId,
  displayName: Schema.String
})
export type User = typeof User.Type
export type UserEncoded = typeof User.Encoded
```

The exact helper calls should be isolated in `api-gen-effect-v4` so Effect v4 API churn is localized.

### 6.3 Generated errors

Rust:

```rust
#[derive(ApiError, ApiType, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum GetUserError {
    #[api(status = 404)]
    UserNotFound { id: UserId },

    #[api(status = 403)]
    PermissionDenied,
}
```

Conceptual generated TS:

```ts
export class UserNotFound extends Schema.TaggedErrorClass<UserNotFound>()(
  "UserNotFound",
  { id: UserId }
) {}

export class PermissionDenied extends Schema.TaggedErrorClass<PermissionDenied>()(
  "PermissionDenied",
  {}
) {}

export type GetUserError =
  | UserNotFound
  | PermissionDenied
```

If Effect v4 changes helper names, only the backend adapter changes. The generated public semantics remain:

- tagged errors;
- schema-backed payloads;
- catchable with Effect tag-based error handling;
- navigable back to Rust variants.

### 6.4 Generated client errors

Every endpoint can fail with client/runtime boundary errors:

```ts
export type ApiClientError =
  | NetworkError
  | TimeoutError
  | EncodeError
  | DecodeError
  | UnexpectedStatusError
  | RemoteProtocolError
```

These are schema-backed tagged errors too.

Domain errors from Rust and client errors are both in the Effect error channel:

```ts
Effect.Effect<User, GetUserError | ApiClientError, ServerApi>
```

### 6.5 Generated service and Layer

Generated service:

```ts
export class ServerApi extends Context.Tag("server::api")<
  ServerApi,
  ServerApi.Service
>() {}

export namespace ServerApi {
  export interface Service {
    readonly users: {
      readonly getUser: (
        args: users.GetUserArgs
      ) => Effect.Effect<User, users.GetUserError | ApiClientError, never>
    }
  }

  export const layer: (
    config: ServerApiConfig
  ) => Layer.Layer<ServerApi, never, HttpClient | AuthProvider | Tracer>
}
```

Generated endpoint accessors:

```ts
export namespace users {
  export interface GetUserArgs {
    readonly id: UserId
  }

  export const getUser = (
    args: GetUserArgs
  ): Effect.Effect<User, GetUserError | ApiClientError, ServerApi> =>
    Effect.flatMap(ServerApi, (api) => api.users.getUser(args))
}
```

### 6.6 Optional Effect HttpApi/RPC metadata

The generator may also emit internal metadata:

```ts
export const GeneratedHttpApi = /* Effect HttpApi representation */
export const GeneratedRpcGroup = /* Effect RPC representation */
```

This should not be required for normal consumers. It exists for interop, documentation, testing, or future tooling.

## 7. API IR

The API IR is the shared contract between Rust collection, TS generation, LSP, usage indexing, and linting.

### 7.1 Top-level IR

```rust
pub struct ApiContract {
    pub workspace: WorkspaceId,
    pub crates: Vec<ApiCrate>,
    pub modules: Vec<ApiModule>,
    pub endpoints: Vec<Endpoint>,
    pub types: Vec<TypeDef>,
    pub errors: Vec<ErrorDef>,
    pub links: Vec<CrossLangLink>,
}
```

### 7.2 Endpoint IR

```rust
pub struct Endpoint {
    pub id: SymbolId,
    pub rust_path: RustPath,
    pub rust_name: String,
    pub ts_path: TsPath,
    pub route: RoutePattern,
    pub method: HttpMethod,
    pub transport: Transport,
    pub request: RequestShape,
    pub response: ResponseShape,
    pub errors: Vec<ErrorRef>,
    pub source: SourceRange,
    pub allow_unused: bool,
}
```

### 7.3 Transport IR

```rust
pub enum Transport {
    UnaryHttp,
    ServerSentEvents,
    WebSocketDuplex,
    BinaryDownload,
    BinaryUpload,
}
```

MVP implements `UnaryHttp` and `ServerSentEvents`.

### 7.4 Type IR

```rust
pub enum TypeShape {
    Primitive(Primitive),
    Struct(StructShape),
    Enum(EnumShape),
    Newtype(TypeRef),
    Tuple(Vec<TypeRef>),
    List(TypeRef),
    Map { key: TypeRef, value: TypeRef },
    Option(TypeRef),
    External(ExternalType),
}
```

### 7.5 Field IR

```rust
pub struct Field {
    pub id: SymbolId,
    pub rust_name: String,
    pub wire_name: String,
    pub ts_name: String,
    pub type_ref: TypeRef,
    pub optionality: Optionality,
    pub rust_source: SourceRange,
}
```

### 7.6 Error IR

```rust
pub struct ErrorVariant {
    pub id: SymbolId,
    pub rust_name: String,
    pub tag: String,
    pub status: HttpStatus,
    pub fields: Vec<Field>,
    pub rust_source: SourceRange,
}
```

### 7.7 Cross-language links

```rust
pub struct CrossLangLink {
    pub symbol_id: SymbolId,
    pub kind: SymbolKind,
    pub rust: RustLocation,
    pub generated_ts: TsLocation,
    pub public_ts_path: TsPath,
}
```

All LSP features use `SymbolId`. They must not infer identity from string names alone.

## 8. Rust implementation strategy

### 8.1 `api-core`

Defines stable traits and wrappers:

```rust
pub trait ApiType {
    fn api_type() -> TypeShape;
}

pub trait ApiError {
    fn api_error() -> ErrorShape;
}

pub trait Endpoint {
    fn endpoint() -> EndpointShape;
}
```

Response wrappers:

```rust
pub struct Json<T>(pub T);
pub struct Created<T>(pub T);
pub struct NoContent;
pub struct Binary<T>(pub T);
pub struct Sse<T>(pub T);
```

Extractor wrappers:

```rust
pub struct Path<T>(pub T);
pub struct Query<T>(pub T);
pub struct Body<T>(pub T);
pub struct Header<T>(pub T);
```

### 8.2 Compile-time shape validation

Macros generate trait obligations so unsupported public API shapes fail during Rust compilation.

Conceptual expansion:

```rust
const _: () = {
    fn assert_api_type<T: api::ApiType>() {}
    fn assert_api_error<E: api::ApiError>() {}

    assert_api_type::<User>();
    assert_api_error::<GetUserError>();
};
```

For obvious local macro errors, emit direct `compile_error!` diagnostics.

Examples of rejected endpoint shapes:

```rust
Result<Json<User>, anyhow::Error>
Result<Json<User>, Box<dyn std::error::Error>>
Result<Json<User>, String>
Json<std::net::TcpStream>
```

### 8.3 `api-macros`

Provides:

```rust
#[derive(ApiType)]
#[derive(ApiError)]
#[api(GET, "/path/{id}")]
api_module! { ... }
```

Responsibilities:

- validate local syntax;
- preserve source ranges;
- generate trait impls;
- generate endpoint metadata;
- generate type/error metadata;
- validate route path parameters against function extractors;
- preserve enough macro output for rust-analyzer and `api-ls` to connect references.

### 8.4 `api-collector`

Compiler-backed collector that reads the built crate graph and produces API IR.

Responsibilities:

- discover root API modules;
- collect reachable endpoints;
- walk transitive DTOs and errors;
- resolve type aliases and newtypes;
- lower Serde attributes;
- detect unsupported public shapes;
- emit source ranges;
- emit stable `SymbolId`s.

Initial implementation can use macro-emitted inventory or generated metadata files. Later implementations can use deeper rustc integration if needed.

## 9. Serde lowering

Serde lowering is mandatory because the TS contract is the wire contract.

MVP support:

- `rename`;
- `rename_all`;
- `tag`;
- `content`;
- externally tagged enums;
- internally tagged enums;
- adjacently tagged enums;
- untagged enums if variants are unambiguous;
- `skip_serializing_if = "Option::is_none"`;
- `default` as optional decode behavior;
- `flatten` for structs/maps where deterministic.

Unsupported Serde forms fail compilation or `api check` with a precise diagnostic.

Examples that may be rejected initially:

- arbitrary custom `serialize_with` / `deserialize_with`;
- untagged enum variants that overlap structurally;
- flatten combinations that cause duplicate fields;
- maps with non-string JSON keys.

## 10. LSP gateway architecture

### 10.1 Gateway process

`api-ls` is the single server registered with editors.

It internally starts:

- rust-analyzer;
- the TypeScript language server with Effect language-service support;
- the API contract compiler/daemon.

```text
Editor
  │ LSP
  ▼
api-ls
  ├─ rust-analyzer
  ├─ TypeScript + Effect language service
  └─ api-contract daemon
```

### 10.2 Request handling policy

For normal Rust features, delegate to rust-analyzer.

For normal TypeScript features, delegate to TypeScript/Effect.

For API-linked symbols:

- rewrite generated TS definitions to Rust source definitions;
- merge Rust and TS references;
- augment hover with route and Effect signature;
- perform cross-language rename;
- surface unused endpoint diagnostics;
- surface stale generated package diagnostics;
- hide generated files unless explicitly requested.

### 10.3 LSP features to implement

MVP:

- `textDocument/definition`;
- `textDocument/typeDefinition`;
- `textDocument/references`;
- `textDocument/hover`;
- `textDocument/rename`;
- `textDocument/prepareRename`;
- `textDocument/publishDiagnostics`;
- `workspace/didChangeWatchedFiles`.

Phase 2:

- `textDocument/codeLens`;
- `callHierarchy/*`;
- `workspace/symbol`;
- `textDocument/documentSymbol` augmentation;
- quick fixes for missing `catchTag` handling;
- quick fixes for `#[api(allow_unused)]`.

### 10.4 TS go-to-definition

For:

```ts
yield* users.getUser({ id })
```

Return Rust endpoint location:

```rust
#[api(GET, "/users/{id}")]
pub async fn get_user(...) -> Result<Json<User>, GetUserError> {
    ...
}
```

Generated TS files should be skipped in the default navigation path.

### 10.5 TS error variant navigation

For:

```ts
Effect.catchTag("UserNotFound", ...)
```

Return Rust error variant location:

```rust
GetUserError::UserNotFound
```

References on the Rust variant should include matching TS `catchTag` and `catchTags` handlers.

### 10.6 Field-level navigation

For:

```ts
user.displayName
```

Return Rust field location:

```rust
pub display_name: String,
```

The mapping uses the symbol graph and Serde lowering.

### 10.7 Rename semantics

#### Endpoint function rename

Rust:

```rust
get_user -> fetch_user
```

TS:

```ts
users.getUser -> users.fetchUser
```

Route path is unchanged unless explicitly edited.

#### Field rename

TS:

```ts
displayName -> fullName
```

Rust should change the wire contract by changing the Rust field where possible:

```rust
pub display_name: String
```

becomes:

```rust
pub full_name: String
```

If Serde container rules cannot express the requested wire name via field naming alone, update Serde attributes to make the new wire name authoritative.

#### Error tag rename

TS:

```ts
Effect.catchTag("UserNotFound", ...)
```

rename to:

```ts
Effect.catchTag("MissingUser", ...)
```

Rust:

```rust
UserNotFound -> MissingUser
```

Update generated class names and error unions on next generation.

## 11. Effect-aware usage analysis

### 11.1 Usage index

The TypeScript/Effect usage index records endpoint references as:

```rust
pub enum UsageStrength {
    Strong,
    Weak,
    Invalid,
    Unknown,
}
```

Each usage includes:

- endpoint symbol id;
- TS file;
- source range;
- usage strength;
- reason;
- associated Effect diagnostics if any.

### 11.2 Strong usages

Count as strong:

```ts
yield* users.getUser({ id })
return users.getUser({ id })
const program = users.getUser({ id }).pipe(...)
Layer.effect(Service, users.getUser({ id }))
Effect.all([users.getUser({ id })])
Stream.fromEffect(users.getUser({ id }))
```

### 11.3 Weak or invalid usages

Do not count as strong:

```ts
users.getUser
users.getUser({ id }) // floating Effect
const f = users.getUser // unless it later composes into a strong usage
```

The analyzer should use TypeScript symbol resolution and Effect diagnostics, not text search.

### 11.4 Rust diagnostics

LSP diagnostic:

```text
warning[api::unused_endpoint]: endpoint is exported but has no strong TypeScript usages
  route: GET /users/{id}
  generated accessor: users.getUser
  help: call it from Effect code, mark #[api(allow_unused)], or remove it
```

Build/CI diagnostic should be added after the LSP diagnostic pipeline is working.

## 12. Compiler/build bridge

The stable compiler-facing bridge should not require a custom rustc.

Initial approach:

1. `api-ls` or `api check-usages` writes `target/api-contract/graph/effect-usage-index.json`.
2. `build.rs` uses `api-build` to read that index.
3. `include_usage_lints!()` includes generated lint glue.
4. Warning mode emits stable Rust warnings where possible.
5. Deny mode emits `compile_error!` for unused endpoints.

Example:

```rust
// build.rs
fn main() {
    api_build::emit_usage_lints();
}
```

```rust
// lib.rs
api::include_usage_lints!();
```

Long-term native lint integration can be added later if exact rustc spans are required.

## 13. Repository layout

Proposed Rust workspace:

```text
crates/
  api-core/
  api-macros/
  api-ir/
  api-collector/
  api-gen-effect-v4/
  api-ls/
  api-build/
  api-axum/
  api-test-fixtures/

npm/
  effect-runtime/
  language-server-wrapper/

examples/
  axum-basic/
  axum-sse/
  monorepo-basic/

docs/
  implementation-plan.md
```

### 13.1 Crates

#### `api-core`

Stable public Rust traits, wrappers, and runtime-independent types.

#### `api-macros`

Proc macros and derive macros.

#### `api-ir`

Shared API contract IR and serialization format.

#### `api-collector`

Rust workspace/crate collector and compiler-backed contract extraction.

#### `api-gen-effect-v4`

Effect v4 TypeScript generator backend.

#### `api-ls`

Gateway LSP server.

#### `api-build`

Build script helper and compiler-facing usage lint bridge.

#### `api-axum`

Axum adapter for converting `ApiModule` into an Axum router.

#### `api-test-fixtures`

Fixture crates and expected generated output.

### 13.2 npm packages

#### `@rust-ts-integration/effect-runtime`

Small generated-client runtime helpers:

- `ApiClientError` definitions;
- fetch transport;
- SSE transport;
- schema encode/decode helpers;
- request construction;
- response decoding.

#### `@rust-ts-integration/language-server`

Optional npm wrapper for installing/running the Rust `api-ls` binary.

## 14. Phased implementation plan

## Phase 0 — Workspace and architecture skeleton

Goal: create the repo skeleton and make all packages build with placeholder APIs.

Deliverables:

- Cargo workspace;
- npm workspace;
- empty crates/packages;
- CI that runs Rust tests, formatting, clippy, TS typecheck, and generated-code snapshot checks;
- this implementation plan in `docs/`.

Exit criteria:

- `cargo test --workspace` passes;
- `cargo clippy --workspace` passes;
- `pnpm test` or equivalent passes;
- a minimal `examples/axum-basic` crate compiles.

## Phase 1 — API IR and Rust public surface

Goal: define the core Rust API and IR without generating TS yet.

Deliverables:

- `api-core` traits and wrappers;
- `api-ir` schema types;
- `#[derive(ApiType)]` for simple structs/newtypes/enums;
- `#[derive(ApiError)]` for enum errors with status codes;
- `#[api(...)]` endpoint macro metadata;
- `api_module!` explicit endpoint root;
- unit tests for IR serialization.

Exit criteria:

- simple endpoint crate emits a valid API IR file;
- unsupported public shapes fail compilation;
- route path params are checked against endpoint inputs.

## Phase 2 — Serde lowering and type validation

Goal: make the generated contract match the Rust wire contract.

Deliverables:

- Serde rename/rename_all support;
- tagged enum support;
- optionality support;
- flatten support for deterministic cases;
- type mappings for common API-layer Rust types;
- precise diagnostics for unsupported Serde/type cases;
- snapshot tests for Rust-to-IR lowering.

Exit criteria:

- DTO snapshots cover structs, enums, newtypes, options, maps, and common external types;
- field-level source ranges are preserved;
- unsupported custom serializers produce actionable errors.

## Phase 3 — Effect v4 generator

Goal: generate a real hidden TypeScript/Effect package from API IR.

Deliverables:

- generated `package.json`;
- generated `index.ts`;
- generated Effect Schema values;
- generated tagged error classes/schemas;
- generated `ServerApi` service and `layer` declarations;
- generated unary HTTP endpoint accessors;
- generated source maps and symbol graph;
- TS snapshot tests.

Exit criteria:

- generated package typechecks with Effect v4;
- generated endpoint accessors have correct `Effect.Effect<A, E, R>` types;
- generated errors are catchable by tag;
- generated schemas distinguish decoded and encoded types.

## Phase 4 — Runtime client and Axum adapter

Goal: make the generated API executable end-to-end for unary JSON HTTP.

Deliverables:

- `@rust-ts-integration/effect-runtime` fetch client;
- request encoder;
- response decoder;
- client error types;
- `api-axum` adapter;
- example Axum server;
- example TS Effect client;
- integration test that starts a server and calls it from generated TS.

Exit criteria:

- `examples/axum-basic` runs end-to-end;
- domain errors appear in the Effect error channel;
- transport/decode errors appear as `ApiClientError`;
- no primary Promise client is exposed.

## Phase 5 — LSP gateway foundation

Goal: make `api-ls` run as the only LSP server and proxy basic Rust/TS behavior.

Deliverables:

- LSP server process;
- config discovery;
- rust-analyzer subprocess management;
- TypeScript/Effect server subprocess management;
- request forwarding;
- virtual/generated file handling;
- diagnostics forwarding;
- integration test harness for LSP requests.

Exit criteria:

- editor can use `api-ls` as sole LSP server;
- normal Rust definitions still work through rust-analyzer;
- normal TS definitions still work through TS/Effect server;
- generated TS package is visible to TS backend.

## Phase 6 — Cross-language navigation

Goal: make generated TS transparent and provide bidirectional definitions/references.

Deliverables:

- Rust↔TS symbol graph format;
- TS endpoint definition -> Rust endpoint;
- TS type definition -> Rust DTO;
- TS field definition -> Rust field;
- TS error tag definition -> Rust error variant;
- Rust endpoint references -> TS strong/weak references plus Rust references;
- Rust DTO/field/error references -> TS references plus Rust references;
- hover augmentation.

Exit criteria:

- go-to-definition from TS endpoint lands in Rust;
- find-references from Rust endpoint includes TS usages;
- field-level navigation works through Serde rename rules;
- generated files are hidden from default navigation.

## Phase 7 — Effect-aware usage index and unused endpoint diagnostics

Goal: classify TypeScript endpoint usages according to Effect semantics.

Deliverables:

- TypeScript AST/symbol usage scanner;
- Effect usage classifier;
- integration with Effect diagnostics where available;
- `effect-usage-index.json`;
- LSP diagnostics for unused endpoints;
- `#[api(allow_unused)]` support;
- CLI `api check-usages`.

Exit criteria:

- floating Effects are not counted as strong usages;
- yielded/composed/returned Effects are counted as strong usages;
- unused endpoint diagnostics appear on Rust endpoint source ranges;
- allow-unused endpoints are ignored by diagnostics.

## Phase 8 — Rename and edits

Goal: support safe cross-language rename.

Deliverables:

- `prepareRename` for endpoints, fields, DTOs, and error variants;
- Rust endpoint rename -> TS accessor usage edits;
- TS accessor rename -> Rust endpoint function rename;
- TS/Rust field rename changes wire contract;
- error tag rename updates Rust variant and TS catchTag/catchTags;
- conflict detection;
- workspace edit tests.

Exit criteria:

- rename operations are deterministic and previewable;
- field rename updates the public wire contract;
- endpoint route path is not changed by function/accessor rename;
- all affected TS usages update.

## Phase 9 — SSE streams

Goal: support one streaming transport in the MVP.

Deliverables:

- Rust SSE endpoint shape;
- stream item/error IR;
- generated `Stream.Stream<A, E, R>` accessors;
- Effect runtime SSE client;
- Axum SSE adapter;
- LSP navigation/references for stream endpoints;
- integration tests.

Exit criteria:

- `examples/axum-sse` streams events to TS client;
- stream item decode errors appear in stream error channel;
- stream endpoint usages are indexed as strong when composed.

## Phase 10 — Build/CI usage lint bridge

Goal: surface unused endpoint warnings/errors outside the editor.

Deliverables:

- `api-build` crate;
- `include_usage_lints!()` macro;
- build script integration;
- warning mode;
- deny mode;
- CI command `api ci` or equivalent.

Exit criteria:

- CI can fail on unused endpoints;
- stale usage index is detected;
- diagnostics clearly identify endpoint route and generated TS accessor.

## 15. Independent task backlog

The tasks below are intentionally scoped so multiple people or agents can implement them independently once the crate/package skeleton exists.

### T-001 — Initialize repository workspace [done]

Goal: create the Rust/npm monorepo skeleton.

Outputs:

- `Cargo.toml` workspace;
- crate directories;
- npm workspace config;
- formatting/lint config;
- CI config;
- placeholder examples.

Dependencies: none.

Acceptance criteria:

- all placeholder crates compile;
- all placeholder npm packages typecheck;
- CI runs the same commands as local development.

### T-002 — Define `api-ir` core data model

Goal: create serializable IR structs/enums.

Outputs:

- `ApiContract`;
- `Endpoint`;
- `TypeDef`;
- `ErrorDef`;
- `SourceRange`;
- `SymbolId`;
- JSON serialization tests.

Dependencies: T-001.

Acceptance criteria:

- IR round-trips through JSON;
- stable symbol IDs are deterministic in tests;
- IR supports unary and SSE transports even if not yet generated.

### T-003 — Implement `api-core` public traits and wrappers

Goal: define user-facing Rust types without macros.

Outputs:

- `ApiType`;
- `ApiError`;
- `Endpoint`;
- `Json<T>`;
- `Created<T>`;
- `NoContent`;
- `Path<T>`;
- `Query<T>`;
- `Body<T>`;
- `ApiModule`.

Dependencies: T-001, T-002.

Acceptance criteria:

- sample code can manually implement traits;
- wrappers are framework-neutral;
- no Axum dependency in `api-core`.

### T-004 — Implement `#[derive(ApiType)]` for simple structs

Goal: derive API type metadata for plain structs.

Outputs:

- derive macro for named-field structs;
- primitive mappings;
- source range capture;
- compile-fail tests.

Dependencies: T-002, T-003.

Acceptance criteria:

- simple DTO derives `ApiType`;
- unsupported field types fail through trait obligations;
- field metadata includes Rust field names.

### T-005 — Implement `#[derive(ApiType)]` for newtypes and enums

Goal: support newtypes and enum wire shapes.

Outputs:

- newtype support;
- unit enum support;
- enum variant metadata;
- basic tagged enum support.

Dependencies: T-004.

Acceptance criteria:

- newtype brands are represented in IR;
- enums preserve variant source ranges;
- unsupported enum forms produce clear diagnostics.

### T-006 — Implement Serde rename lowering

Goal: make field names match the wire contract.

Outputs:

- `rename`;
- `rename_all`;
- field `wire_name`;
- optional field support from Serde omit-if-none.

Dependencies: T-004.

Acceptance criteria:

- `display_name` with camelCase becomes `displayName`;
- field-level source links survive rename lowering;
- snapshot tests cover common rename rules.

### T-007 — Implement `#[derive(ApiError)]`

Goal: define public typed endpoint errors.

Outputs:

- enum-only derive;
- variant status mapping;
- tag extraction;
- payload field metadata;
- compile errors for missing/invalid statuses.

Dependencies: T-002, T-003, T-005, T-006.

Acceptance criteria:

- error enum generates `ErrorDef` IR;
- every variant has a status;
- variants map to TS tag names.

### T-008 — Implement `#[api(...)]` endpoint macro

Goal: mark and validate Rust endpoint functions.

Outputs:

- method/path parser;
- route param validation;
- return type validation;
- endpoint metadata;
- `allow_unused` flag.

Dependencies: T-002, T-003, T-007.

Acceptance criteria:

- async endpoint emits endpoint metadata;
- path params must be present as extractors;
- invalid return/error types fail compilation.

### T-009 — Implement `api_module!`

Goal: explicitly compose exported endpoints.

Outputs:

- macro syntax;
- root module metadata;
- endpoint reachability graph.

Dependencies: T-008.

Acceptance criteria:

- only reachable endpoints are exported;
- router composition produces Rust references;
- missing endpoint symbols fail compilation.

### T-010 — Build first `api-collector`

Goal: produce API IR files from a crate.

Outputs:

- Cargo workspace discovery;
- root module discovery;
- endpoint/type/error collection;
- IR JSON output;
- CLI command `api collect`.

Dependencies: T-002 through T-009.

Acceptance criteria:

- sample crate emits complete IR;
- transitive DTOs are included;
- non-reachable DTOs are not exported.

### T-011 — Implement common external type mappings

Goal: support realistic API-layer Rust types.

Outputs:

- `uuid::Uuid`;
- `chrono::DateTime<Utc>`;
- `rust_decimal::Decimal`;
- integer policy;
- `serde_json::Value`;
- map/list/option support.

Dependencies: T-004, T-010.

Acceptance criteria:

- mappings appear in IR with encoded/decoded distinctions;
- unsupported generic/external types fail with useful diagnostics.

### T-012 — Implement Effect schema generator

Goal: generate Effect Schema values for API types.

Outputs:

- `schemas.ts`;
- decoded type aliases;
- encoded type aliases;
- branded newtypes;
- snapshot tests.

Dependencies: T-002, T-006, T-011.

Acceptance criteria:

- generated schemas typecheck;
- generated names are deterministic;
- decoded/encoded aliases are emitted for every exported type.

### T-013 — Implement Effect error generator

Goal: generate tagged schema-backed error classes/unions.

Outputs:

- `errors.ts`;
- domain error classes;
- error unions;
- status metadata;
- generated client error classes.

Dependencies: T-007, T-012.

Acceptance criteria:

- `Effect.catchTag` works on generated errors;
- error variant source links are in symbol graph;
- statuses are preserved.

### T-014 — Implement endpoint accessor generator

Goal: generate Effect endpoint functions.

Outputs:

- endpoint arg types;
- endpoint accessor functions;
- `Effect.Effect<A, E, ServerApi>` signatures;
- generated route metadata.

Dependencies: T-008, T-012, T-013.

Acceptance criteria:

- generated accessors typecheck;
- Rust `Result<T, E>` maps to Effect error channel;
- request path/query/body args are represented correctly.

### T-015 — Implement generated `ServerApi` service and Layer declarations

Goal: expose a generated Effect service API.

Outputs:

- `ServerApi` context tag;
- service interface;
- `ServerApi.layer` declaration;
- generated mocks/test layer helpers.

Dependencies: T-014.

Acceptance criteria:

- endpoint accessors require `ServerApi`;
- service methods internally require `never`;
- TS examples compose with `Effect.provide`.

### T-016 — Implement generated package resolver

Goal: make TS projects import hidden generated packages.

Outputs:

- generated `package.json`;
- path mapping metadata;
- `api init-tsconfig` helper or documented snippet;
- cache path conventions.

Dependencies: T-012 through T-015.

Acceptance criteria:

- `import { users } from "@workspace/server-api"` resolves;
- no generated files need to be committed;
- multiple TS projects can share the same generated package.

### T-017 — Implement runtime fetch client

Goal: execute unary HTTP endpoints as Effects.

Outputs:

- request encoding;
- response decoding;
- client error mapping;
- fetch transport;
- timeout support.

Dependencies: T-012 through T-015.

Acceptance criteria:

- success responses decode to success channel;
- domain errors decode to error channel;
- transport/decode errors are `ApiClientError`.

### T-018 — Implement Axum adapter

Goal: serve Rust endpoints through Axum.

Outputs:

- `api-axum` crate;
- `ApiModule` to Axum router conversion;
- JSON success/error response handling;
- status code mapping.

Dependencies: T-008, T-009, T-017.

Acceptance criteria:

- example Axum app serves generated client;
- Rust domain errors become expected TS Effect errors;
- route params/body/query are decoded correctly.

### T-019 — Implement API symbol graph

Goal: connect Rust and generated TS locations.

Outputs:

- `rust-ts-symbols.json`;
- endpoint links;
- type links;
- field links;
- error variant links;
- generated TS source ranges.

Dependencies: T-010, T-012 through T-015.

Acceptance criteria:

- every generated public TS symbol has a Rust source link where applicable;
- field links account for Serde rename;
- symbol graph round-trips through JSON.

### T-020 — Implement `api-ls` process and config discovery

Goal: create the gateway LSP executable.

Outputs:

- LSP server bootstrap;
- workspace root discovery;
- config loading;
- logging;
- generated cache awareness.

Dependencies: T-001.

Acceptance criteria:

- editor can start `api-ls`;
- `initialize` and `shutdown` work;
- config errors are reported clearly.

### T-021 — Proxy rust-analyzer through gateway

Goal: preserve Rust language features.

Outputs:

- subprocess lifecycle;
- request forwarding;
- response forwarding;
- diagnostics forwarding.

Dependencies: T-020.

Acceptance criteria:

- normal Rust go-to-definition works through `api-ls`;
- normal Rust diagnostics appear;
- rust-analyzer crashes are handled gracefully.

### T-022 — Proxy TypeScript/Effect server through gateway

Goal: preserve TS and Effect language features.

Outputs:

- subprocess lifecycle;
- generated package file serving;
- request forwarding;
- diagnostics forwarding;
- Effect language-service plugin configuration.

Dependencies: T-016, T-020.

Acceptance criteria:

- normal TS go-to-definition works through `api-ls`;
- Effect diagnostics appear;
- generated packages are visible to TS backend.

### T-023 — Implement cross-language definition

Goal: redirect TS definitions to Rust.

Outputs:

- TS endpoint -> Rust endpoint;
- TS type -> Rust DTO;
- TS field -> Rust field;
- TS error tag -> Rust variant.

Dependencies: T-019, T-021, T-022.

Acceptance criteria:

- default navigation never lands in generated TS for linked API symbols;
- definitions are precise at field/variant level;
- non-API TS symbols still use TS backend normally.

### T-024 — Implement cross-language references

Goal: merge Rust and TS references.

Outputs:

- Rust endpoint refs include TS usages;
- TS endpoint refs include Rust endpoint/router refs;
- field refs are type-aware;
- error variant refs include `catchTag`/`catchTags`.

Dependencies: T-019, T-023.

Acceptance criteria:

- references are not based on plain text search;
- generated-file references are hidden by default;
- duplicate references are removed.

### T-025 — Implement hover augmentation

Goal: show combined Rust/API/Effect metadata.

Outputs:

- endpoint hover route/method;
- Effect signature;
- domain error list;
- source crate/module;
- usage count placeholder.

Dependencies: T-019, T-023.

Acceptance criteria:

- TS endpoint hover shows route and Rust source;
- Rust endpoint hover shows generated TS accessor and Effect signature.

### T-026 — Implement Effect usage scanner

Goal: classify TS endpoint usages.

Outputs:

- AST/symbol-based reference scanner;
- strong/weak/invalid/unknown usage classification;
- integration with Effect diagnostics when available;
- `effect-usage-index.json`.

Dependencies: T-014, T-019, T-022.

Acceptance criteria:

- yielded/composed Effects count as strong;
- floating Effects do not count as strong;
- import-only references do not count as strong.

### T-027 — Implement unused endpoint LSP diagnostics

Goal: surface unused endpoint warnings in Rust source.

Outputs:

- diagnostic generation;
- `#[api(allow_unused)]` filter;
- config for warn/deny/off;
- tests.

Dependencies: T-026.

Acceptance criteria:

- endpoints with no strong TS usages produce diagnostics;
- endpoints with strong usages do not;
- allow-unused suppresses diagnostics.

### T-028 — Implement cross-language rename

Goal: support bidirectional API refactors.

Outputs:

- `prepareRename`;
- endpoint function/accessor rename;
- field/wire contract rename;
- error variant/tag rename;
- workspace edit builder;
- conflict detection.

Dependencies: T-019, T-023, T-024.

Acceptance criteria:

- renaming TS field changes Rust wire contract;
- renaming Rust endpoint updates TS usages;
- route path is not changed by function rename;
- edits are previewable and deterministic.

### T-029 — Implement SSE IR and generator

Goal: support server streams.

Outputs:

- stream endpoint IR;
- stream item/error lowering;
- generated `Stream.Stream<A, E, R>` accessors;
- symbol graph links.

Dependencies: T-008, T-012 through T-015.

Acceptance criteria:

- SSE endpoint typechecks in generated TS;
- stream errors are in the stream error channel;
- LSP navigation works for stream endpoints.

### T-030 — Implement SSE runtime and Axum adapter

Goal: make server streams executable.

Outputs:

- Effect SSE client;
- Axum SSE response adapter;
- stream decode errors;
- integration example.

Dependencies: T-018, T-029.

Acceptance criteria:

- TS client consumes Rust SSE stream as `Stream.Stream`;
- dropped/failed streams surface typed errors;
- integration test passes.

### T-031 — Implement build usage lint bridge

Goal: surface unused endpoints outside the editor.

Outputs:

- `api-build` helper;
- generated lint glue;
- warning mode;
- deny mode;
- stale index detection.

Dependencies: T-026, T-027.

Acceptance criteria:

- CI can fail on unused endpoints;
- build warnings identify route/accessor;
- stale/missing index reports actionable instructions.

### T-032 — Implement snapshot and fixture test suite

Goal: keep output deterministic and safe to refactor.

Outputs:

- Rust fixture crates;
- generated TS snapshots;
- symbol graph snapshots;
- LSP transcript tests;
- compile-fail tests.

Dependencies: can start after T-001, expands continuously.

Acceptance criteria:

- every supported feature has a fixture;
- generator output is deterministic;
- LSP features are covered by protocol-level tests.

### T-033 — Implement developer CLI

Goal: provide a unified command-line interface.

Outputs:

- `api collect`;
- `api gen`;
- `api watch`;
- `api check`;
- `api check-usages`;
- `api doctor`.

Dependencies: T-010, T-016, T-026.

Acceptance criteria:

- CLI can regenerate hidden package;
- CLI can validate stale cache;
- CLI can explain missing setup.

### T-034 — Documentation and examples

Goal: make the system understandable and usable.

Outputs:

- getting started guide;
- Axum example guide;
- Effect client guide;
- LSP setup guide;
- rename/unused endpoint behavior docs;
- limitations page.

Dependencies: can start after T-018, expands continuously.

Acceptance criteria:

- new user can create an endpoint and call it from Effect TS;
- docs explain why generated files are hidden;
- docs explain error-channel semantics.

## 16. Parallelization plan

After T-001 is complete, work can split into these tracks.

### Track A — Rust contract

- T-002
- T-003
- T-004
- T-005
- T-006
- T-007
- T-008
- T-009
- T-010
- T-011

### Track B — Effect generation/runtime

Can begin once basic IR exists.

- T-012
- T-013
- T-014
- T-015
- T-016
- T-017

### Track C — Server integration

Can begin after endpoint macro/module basics.

- T-018
- T-029
- T-030

### Track D — LSP gateway

Can begin early with mocked symbol graphs.

- T-020
- T-021
- T-022
- T-023
- T-024
- T-025
- T-028

### Track E — Usage/lints

Can begin after generated endpoint accessors and symbol graph exist.

- T-026
- T-027
- T-031

### Track F — Testing/docs/tooling

Continuous.

- T-032
- T-033
- T-034

## 17. Suggested milestone order

### Milestone 1: Compile-time Rust contract

Complete:

- T-001 through T-010.

Result:

- Rust endpoints can be marked;
- DTOs/errors can be derived;
- API IR can be emitted;
- bad API shapes fail early.

### Milestone 2: Typechecked generated Effect package

Complete:

- T-011 through T-016.

Result:

- hidden TS package is generated;
- `import { users } from "@workspace/server-api"` resolves;
- endpoint accessors have correct Effect types.

### Milestone 3: End-to-end unary JSON

Complete:

- T-017 and T-018.

Result:

- Axum server and Effect client work together;
- domain errors are in the Effect error channel.

### Milestone 4: LSP transparent definitions/references

Complete:

- T-019 through T-025.

Result:

- TS go-to-definition jumps to Rust;
- Rust find-references includes TS usages;
- field-level navigation works.

### Milestone 5: Usage warnings

Complete:

- T-026, T-027, T-031.

Result:

- unused endpoints are detected based on strong Effect usage;
- CI can deny unused endpoints.

### Milestone 6: Rename and streams

Complete:

- T-028 through T-030.

Result:

- cross-language rename works;
- wire contract rename is supported;
- SSE streams work end-to-end.

## 18. Key risks and mitigations

### 18.1 Effect v4 API churn

Risk: generated code breaks as Effect v4 evolves.

Mitigation:

- isolate all Effect-specific syntax in `api-gen-effect-v4`;
- snapshot generated TS;
- pin supported Effect versions;
- use a small runtime adapter package;
- keep public generated API stable even if internals change.

### 18.2 LSP gateway complexity

Risk: proxying two language servers and rewriting results is complex.

Mitigation:

- build gateway first with transparent forwarding only;
- add cross-language behavior feature by feature;
- create protocol-level tests with recorded LSP requests/responses;
- keep generated symbol graph deterministic.

### 18.3 Rust source ranges from macros

Risk: proc macro expansion loses precise source locations.

Mitigation:

- capture source spans in macro inputs wherever possible;
- store explicit source ranges in generated metadata;
- use rust-analyzer queries for final source lookup where possible;
- write snapshot tests for every symbol kind.

### 18.4 Serde compatibility

Risk: matching Serde exactly is hard.

Mitigation:

- start with explicit supported subset;
- reject unsupported forms loudly;
- add fixture tests for each Serde feature;
- prefer compile failure over generating wrong TS.

### 18.5 Usage analysis false positives/negatives

Risk: deciding whether an endpoint is truly used can be subtle.

Mitigation:

- classify usage strength rather than boolean usage;
- count only strong usage for warnings;
- expose `api explain-usage endpoint`;
- support `#[api(allow_unused)]`;
- keep CI behavior configurable.

## 19. Initial file checklist

After this document, the next repository commits should create:

```text
Cargo.toml
crates/api-core/Cargo.toml
crates/api-core/src/lib.rs
crates/api-ir/Cargo.toml
crates/api-ir/src/lib.rs
crates/api-macros/Cargo.toml
crates/api-macros/src/lib.rs
crates/api-collector/Cargo.toml
crates/api-gen-effect-v4/Cargo.toml
crates/api-ls/Cargo.toml
crates/api-build/Cargo.toml
crates/api-axum/Cargo.toml
npm/package.json
npm/effect-runtime/package.json
examples/axum-basic/Cargo.toml
```

The first implementation target should be Milestone 1: compile-time Rust contract and API IR emission.
