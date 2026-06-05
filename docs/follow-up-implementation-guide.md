# Follow-up implementation guide

This document reviews the current implementation against the original target and lays out the remaining work required to fully realize the vision:

- Rust is the source of truth for API endpoints, DTOs, errors, routes, and streams.
- The TypeScript surface is Effect v4-native: `Effect.Effect<A, E, R>`, `Stream.Stream<A, E, R>`, generated Schema, tagged errors, service accessors, and Layers.
- Generated TypeScript files are hidden disposable cache artifacts, not hand-authored source.
- `api-ls` runs in gateway mode only. It owns cross-language definition, references, hover, rename, diagnostics, and usage indexing.
- Field rename is a wire-contract rename. It updates the Rust API contract, not merely a local Serde compatibility shim.
- Unsupported public API shapes fail Rust compilation.
- Exported endpoints with no live Effect usage are reported in the editor and can be warned/denied in build/CI.

## Review summary

The current repository has a useful foundation: workspace crate layout, `ApiType` / `ApiError` / `#[api]` macros, an IR crate, an Effect v4 generator, an Axum adapter, an Effect runtime package, a CLI, a usage-lint bridge, and a gateway-shaped LSP crate.

The main gaps are not file organization. The real gaps are semantic:

1. Collection is not automatic enough. The current CLI `api collect` writes an empty contract, and realistic examples still pass `types` and `errors` manually. The original vision requires endpoint-root-driven, transitive, compiler-backed collection.
2. Source mapping is not precise enough for field-level navigation and rename. Most Rust source locations are placeholder/call-site ranges, and the IR does not yet store TypeScript generated ranges.
3. Serde compatibility is too shallow. The macros cover a small subset of rename behavior and do not yet model flattening, tagging modes, defaults, skip behavior, aliases, transparent wrappers, custom serializers, or unsupported-shape diagnostics with precise spans.
4. The generated Effect package needs to be validated by real TypeScript/Effect typechecking. It currently generates plausible TypeScript, but there is no end-to-end generated-package typecheck fixture that proves the emitted Effect v4 code compiles and behaves as intended.
5. The usage index is line/string based. It does not yet use TypeScript AST references, TypeScript symbol resolution, or Effect language-service diagnostics, so it will produce false positives and false negatives.
6. The LSP gateway is structurally present, but it needs a production async proxy core, backend request-id mapping, capability merging, semantic symbol lookup, Effect-aware TypeScript backend integration, and full cross-language feature coverage.
7. The npm language-server wrapper is not an executable launcher yet.
8. Rename behavior is documented but not complete. Wire-contract rename requires coordinated Rust edits, Serde rewrite rules, TS usage edits, generated-cache invalidation, and conflict checks.

## Definition of done

The implementation is complete when this works in a fresh monorepo without editing generated files:

```rust
#[derive(Clone, Debug, Serialize, Deserialize, ApiType)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: UserId,
    pub display_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ApiError)]
#[serde(tag = "_tag", rename_all = "PascalCase")]
pub enum GetUserError {
    #[api_error(status = 404)]
    UserNotFound { id: UserId },
}

#[api(method = "GET", path = "/users/{id}")]
pub async fn get_user(id: Path<UserId>) -> Result<Json<User>, GetUserError> {
    todo!()
}

pub fn api() -> ApiModule {
    api_module!(name = "server", endpoints = [get_user])
}
```

and TypeScript receives:

```ts
import { Effect } from "effect"
import { ServerApi, users } from "@workspace/server-api"

const program = Effect.gen(function* () {
  const user = yield* users.getUser({ id })
  return user.displayName
}).pipe(
  Effect.catchTag("UserNotFound", () => Effect.succeed(undefined)),
  Effect.provide(ServerApi.layer({ baseUrl: "/api" }))
)
```

Required behavior:

- `cargo api collect` discovers endpoints, request types, response types, stream item types, and errors transitively from the Rust API root.
- `cargo api gen` materializes a hidden Effect package under `target/api-contract/effect-v4/packages/...`.
- `cargo api check` runs collection, generation, generated-package typecheck, usage indexing, and lint validation.
- `api-ls` is the only language server entry point for this workspace and delegates to rust-analyzer plus the Effect-aware TypeScript backend.
- TS go-to-definition on `users.getUser`, `User`, `displayName`, and `catchTag("UserNotFound")` jumps to the Rust endpoint, type, field, or error variant.
- Rust find-references on the endpoint includes TS Effect call sites.
- Rust find-references on the error variant includes TS `catchTag` / `catchTags` handling sites.
- Rust find-references on a DTO field includes TS property reads/writes.
- Rename from either Rust or TS updates the wire contract and all relevant Rust/TS usages.
- Unused endpoint diagnostics count only strong/live Effect usages.
- Unsupported endpoint shapes fail rustc with actionable diagnostics.

---

# Track A: make contract collection automatic and compiler-backed

## Task A1: replace manual `CollectorInput { types, errors }` with transitive type collection

### Current gap

The current collector requires explicit `types` and `errors` lists in realistic examples. That defeats the low-code goal and will not scale to multiple crates.

### Implementation

Extend `ApiType` and `ApiError` with recursive registration hooks:

```rust
pub trait ApiType {
    const RUST_NAME: &'static str;
    const TS_NAME: &'static str = Self::RUST_NAME;

    fn type_ref() -> TypeRef;
    fn type_def() -> TypeDef;

    fn register_types(registry: &mut TypeRegistry) {
        registry.insert(Self::type_def());
    }
}

pub trait ApiError: ApiType {
    fn error_ref() -> ErrorRef;
    fn error_def() -> ErrorDef;

    fn register_error(registry: &mut ErrorRegistry) {
        registry.insert(Self::error_def());
        Self::register_types(registry.types_mut());
    }
}
```

The derive macro for a struct should emit:

```rust
fn register_types(registry: &mut TypeRegistry) {
    if registry.insert(Self::type_def()) {
        <Field1 as ApiType>::register_types(registry);
        <Field2 as ApiType>::register_types(registry);
    }
}
```

The endpoint metadata function generated by `#[api]` should include enough typed registration logic to collect all reachable request, response, stream, and error types.

Add an `EndpointDescriptor` type instead of returning only plain `Endpoint`:

```rust
pub struct EndpointDescriptor {
    pub endpoint: Endpoint,
    pub register: fn(&mut ContractRegistry),
}
```

For compatibility, `ApiModule` can store descriptors and expose `endpoint_irs()`.

### Acceptance criteria

- The monorepo example no longer manually passes `types: vec![...]` or `errors: vec![...]`.
- `collect_contract` takes only package metadata and the root `ApiModule`.
- Nested DTOs are emitted automatically.
- Error-variant payload DTOs are emitted automatically.
- `Vec<T>`, `Option<T>`, maps, newtypes, stream items, request bodies, path params, and query params collect their inner types automatically.

### Independent test cases

- Endpoint returns `Json<User>` where `User` contains `UserId` newtype.
- Endpoint body is `Body<CreateUser>` and response is `Json<User>`.
- Error variant contains a DTO field.
- SSE endpoint uses `Sse<Event, Stream<Item = Result<Event, EventError>>>`.

## Task A2: implement a real `api collect` command

### Current gap

`api collect` currently creates an empty contract. It can inspect Cargo metadata, but it does not invoke the user API root or collect real endpoints.

### Implementation

Provide one supported collection path:

```sh
cargo api collect --package server --root server::api --out target/api-contract/server-api.json
```

Use a generated collector binary approach:

1. Resolve package and target with `cargo metadata`.
2. Generate a temporary collector crate under `target/api-contract/collector/<package>/`.
3. Depend on the user package by path.
4. Call the configured root function, for example `server::api()`.
5. Serialize the resulting `ApiContract`.
6. Write `target/api-contract/<package>.json` and the symbol graph seed.

Generated collector main:

```rust
fn main() {
    let module = server::api();
    let contract = api_collector::collect_contract(api_collector::CollectorInput {
        package_name: "@workspace/server-api".to_owned(),
        root_module: module,
    });
    println!("{}", api_collector::contract_to_json(&contract).unwrap());
}
```

The command must run through Cargo so that proc macros, feature flags, cfgs, and dependency versions match the real workspace.

### Acceptance criteria

- `api collect` no longer has an empty-contract path except for an explicit `--empty` test/debug flag.
- It can collect from a workspace package by package name and root function.
- It respects package features.
- It fails with a clear message if the root function is missing or has the wrong type.

## Task A3: support package metadata for TS package naming

### Implementation

Read:

```toml
[package.metadata.rust_ts]
ts_package = "@workspace/server-api"
api_root = "server::api"
```

Fallback naming rule:

```text
Cargo package `server-api` -> `@generated/server-api`
```

### Acceptance criteria

- No required CLI package-name argument when metadata exists.
- Multiple API crates produce multiple generated TS packages.
- Shared Rust crates can produce generated shared packages or be inlined according to config.

## Task A4: build a workspace contract graph

### Implementation

Add a workspace-level file:

```text
target/api-contract/workspace-contract.json
```

with:

```json
{
  "packages": [
    {
      "cargoPackage": "server",
      "tsPackage": "@workspace/server-api",
      "contract": "target/api-contract/server.json"
    }
  ],
  "dependencies": []
}
```

### Acceptance criteria

- Generated package imports can reference another generated package instead of duplicating shared DTOs.
- `api check` can validate all API packages in dependency order.

---

# Track B: upgrade the IR and symbol graph

## Task B1: introduce IR v2 with explicit channels and response metadata

### Current gap

The current `ResponseShape` is too small for the intended API surface. It does not distinguish success status, headers, created/no-content responses, binary payloads, auth middleware errors, or Effect requirements.

### Implementation

Add:

```rust
pub struct ApiContractV2 {
    pub schema_version: u32,
    pub package: PackageId,
    pub dependencies: Vec<PackageDependency>,
    pub endpoints: Vec<EndpointV2>,
    pub types: Vec<TypeDefV2>,
    pub errors: Vec<ErrorDefV2>,
    pub links: Vec<SymbolLink>,
}

pub struct EndpointV2 {
    pub id: SymbolId,
    pub rust_path: Vec<String>,
    pub ts_path: Vec<String>,
    pub route: RoutePattern,
    pub method: HttpMethod,
    pub transport: Transport,
    pub request: RequestShapeV2,
    pub success: SuccessShape,
    pub error_channel: Vec<ErrorRef>,
    pub requirements: Vec<RequirementRef>,
    pub source: SourceRange,
    pub allow_unused: bool,
}

pub enum SuccessShape {
    Empty { status: u16 },
    Json { status: u16, ty: TypeRef },
    Binary { status: u16, media_type: Option<String> },
    Stream { item: TypeRef },
}
```

### Acceptance criteria

- `Json<T>` maps to JSON 200.
- `Created<T>` maps to JSON 201.
- `NoContent` maps to 204 and TS `void` / `undefined` success.
- SSE endpoints map to `Stream` success.
- Future binary upload/download wrappers can be represented without breaking schema.

## Task B2: store Rust and generated TS ranges for every symbol

### Current gap

The current IR contains Rust source ranges but not generated TS ranges, and the macro-produced Rust ranges are not precise enough for fields/variants.

### Implementation

Add a symbol graph file:

```text
target/api-contract/graph/rust-ts-symbols.json
```

Shape:

```json
{
  "schemaVersion": 2,
  "symbols": [
    {
      "id": "...",
      "kind": "field",
      "rust": {
        "file": "crates/server/src/users.rs",
        "nameRange": { "start": { "line": 10, "character": 8 }, "end": { "line": 10, "character": 20 } },
        "fullRange": { "start": { "line": 10, "character": 4 }, "end": { "line": 10, "character": 28 } }
      },
      "typescript": [
        {
          "file": "target/api-contract/effect-v4/packages/_workspace_server-api/schemas.ts",
          "nameRange": { "start": { "line": 8, "character": 2 }, "end": { "line": 8, "character": 13 } },
          "fullRange": { "start": { "line": 8, "character": 2 }, "end": { "line": 8, "character": 28 } }
        }
      ],
      "metadata": {
        "rustName": "display_name",
        "wireName": "displayName",
        "tsName": "displayName"
      }
    }
  ]
}
```

### Implementation notes

- For Rust ranges, parse the source file with `syn` in the derive macro crate only if `span-locations` are available in the build path, or better, have the collector parse the user source files with `syn` / rust-analyzer line index and match items by module path.
- Do not rely on `file!()`, `line!()`, and `column!()` from generated metadata for field-level ranges.
- For TS ranges, have the generator write through a `TrackedWriter` that records offsets when it renders every exported type, field, endpoint accessor, error class, error tag, and namespace.

### Acceptance criteria

- TS go-to-definition on a generated type lands on the Rust type identifier.
- TS go-to-definition on a generated field lands on the Rust field identifier.
- TS `catchTag("Variant")` lands on the Rust error variant.
- Generated file ranges are stable across deterministic re-generation.

## Task B3: model encoded and decoded TypeScript types in IR

### Implementation

Add explicit mapping metadata:

```rust
pub struct TypeDefV2 {
    pub decoded_ts_name: String,
    pub encoded_ts_name: String,
    pub schema_name: String,
    pub shape: TypeShapeV2,
}
```

This allows:

```text
uuid::Uuid                  encoded string, decoded branded string
DateTime<Utc>               encoded ISO string, decoded configured DateTime/string
Decimal                     encoded string, decoded configured Decimal/string
newtype                     encoded inner, decoded brand
```

### Acceptance criteria

- Generated code exports `User`, `UserEncoded`, and `UserSchema` consistently.
- Request encoding uses the encoded shape.
- Response decoding returns the decoded shape.

---

# Track C: make Serde lowering correct enough for real APIs

## Task C1: support the common Serde attribute matrix

### Current gap

The current macros only cover a narrow rename subset. Real API DTOs need broader Serde support.

### Required support

Struct/container:

```rust
#[serde(rename = "...")]
#[serde(rename_all = "camelCase")]
#[serde(tag = "type")]
#[serde(tag = "type", content = "data")]
#[serde(untagged)]
#[serde(transparent)]
#[serde(default)]
```

Field:

```rust
#[serde(rename = "...")]
#[serde(alias = "...")]
#[serde(default)]
#[serde(skip)]
#[serde(skip_serializing)]
#[serde(skip_deserializing)]
#[serde(skip_serializing_if = "Option::is_none")]
#[serde(flatten)]
```

Variant:

```rust
#[serde(rename = "...")]
#[serde(alias = "...")]
#[serde(skip)]
```

### Acceptance criteria

- Externally tagged, internally tagged, adjacently tagged, and untagged enums generate correct Effect schemas.
- `flatten` either generates a correct object spread/intersection schema or fails with a clear diagnostic when ambiguous.
- `skip` fields do not appear in the API shape.
- `skip_serializing_if = "Option::is_none"` produces optional encoded fields.
- Unsupported custom serializers fail with a targeted error unless the field has an explicit API override.

## Task C2: fail custom serialization explicitly

### Implementation

Reject these by default on public API DTOs:

```rust
#[serde(serialize_with = "...")]
#[serde(deserialize_with = "...")]
#[serde(with = "...")]
```

unless the field also has:

```rust
#[api(type = "SomeTsType", schema = "SomeSchema")]
```

or a Rust-side external type mapping exists.

### Acceptance criteria

- The compiler fails at the exact field with help text.
- There are UI tests for each unsupported custom serializer path.

## Task C3: implement complete rename rules

### Implementation

Support at least:

```text
lowercase
UPPERCASE
PascalCase
camelCase
snake_case
SCREAMING_SNAKE_CASE
kebab-case
SCREAMING-KEBAB-CASE
```

### Acceptance criteria

- No `unsupported serde rename_all rule lowercase` failure for standard Serde rules.
- UI tests cover every rule.

---

# Track D: make generated Effect v4 code executable and typechecked

## Task D1: add generated-package golden tests plus real TypeScript typecheck

### Current gap

The generator has Rust tests, but there is no fixture that writes the generated package, installs the runtime package, and runs TypeScript against the generated code.

### Implementation

Create:

```text
tests/generated-effect-basic/
  contract.json
  app/
    package.json
    tsconfig.json
    src/index.ts
```

Test command:

```sh
cargo run -p api-collector --bin api -- gen \
  --contract tests/generated-effect-basic/contract.json \
  --target-dir tests/generated-effect-basic/target

npm --prefix tests/generated-effect-basic/app test
```

The app should import the generated package and compile real Effect code:

```ts
const program = Effect.gen(function* () {
  const user = yield* users.getUser({ id })
  return user.displayName
}).pipe(
  Effect.catchTag("UserNotFound", () => Effect.succeed("missing")),
  Effect.provide(ServerApi.layer({ baseUrl: "/api", fetch: mockFetch }))
)
```

### Acceptance criteria

- Generated package compiles with strict TypeScript.
- Generated endpoint accessors expose the expected `Effect.Effect<A, E, R>` shape.
- Generated errors are catchable by tag.
- Generated stream endpoints expose `Stream.Stream<A, E, R>`.

## Task D2: fix generated package publishing semantics

### Implementation

The generated package should not use `"types": "./index.ts"` as the final package contract. Emit either:

```text
index.ts + compiled dist/index.js + dist/index.d.ts
```

or, for hidden dev cache only:

```text
index.ts with tsconfig paths and no package.json `types` field claiming publishable semantics
```

Recommended hidden-cache layout:

```text
target/api-contract/effect-v4/packages/_workspace_server-api/
  package.json
  src/index.ts
  src/schemas.ts
  src/errors.ts
  src/endpoints.ts
  src/layer.ts
  tsconfig.json
```

For publish mode:

```text
dist/index.js
dist/index.d.ts
dist/index.d.ts.map
```

### Acceptance criteria

- Dev-cache import works through `tsconfig.paths`.
- Publish mode emits standard JS and declarations.
- The generated package is not accidentally presented as a publishable package when it only contains TS source.

## Task D3: use Effect Schema for both request encoding and response decoding

### Implementation

Endpoint metadata should include schemas:

```ts
const getUserEndpoint = {
  method: "GET",
  path: "/users/{id}",
  pathSchema: Schema.Struct({ id: UserId }),
  successSchema: User,
  errorSchemas: [UserNotFound, PermissionDenied]
}
```

Runtime calls should:

1. Encode path/query/body through schemas.
2. Decode success bodies through schemas.
3. Decode errors by status and tag.
4. Fail with `DecodeError` / `EncodeError` in the Effect error channel.

### Acceptance criteria

- Request bodies are encoded according to `Encoded` types.
- Responses are decoded into `Type` types.
- Invalid response JSON fails with `DecodeError`, not an untyped thrown exception.

## Task D4: complete Effect runtime protocol tests

### Required tests

- successful unary JSON request
- domain error by status and `_tag`
- unexpected status
- invalid success body
- invalid error body
- request encoding failure
- timeout
- fetch/network failure
- 204 no content
- 201 created
- query parameters with arrays
- path encoding
- SSE success events
- SSE `api-error` event
- SSE invalid JSON frame
- SSE cancellation/reader release

### Acceptance criteria

- Runtime tests run under Node with mock `fetch` and mock `ReadableStream`.
- The generated client never exposes raw thrown errors for expected failures.

---

# Track E: make Axum integration safe and ergonomic

## Task E1: remove route duplication and validate handler pairing

### Current gap

`ApiRouter::route(endpoint, method_router(...))` pairs arbitrary endpoint metadata with arbitrary handler routers. It is possible to route endpoint A to handler B.

### Implementation

Generate a typed handler adapter from `#[api]`:

```rust
#[api(method = "GET", path = "/users/{id}")]
pub async fn get_user(id: Path<UserId>) -> Result<Json<User>, GetUserError> { ... }
```

should generate:

```rust
pub fn get_user_endpoint() -> EndpointDescriptor { ... }
pub fn get_user_route<S>() -> ApiRoute<S> { ... }
```

Then router composition can be:

```rust
api_axum::router(api()).routes([
    get_user_route(),
    create_user_route(),
])
```

or:

```rust
api_axum::serve(api())
```

when all endpoints are in the root module.

### Acceptance criteria

- The route path and HTTP method are not duplicated by the user.
- The handler and endpoint descriptor are generated from the same Rust function.
- Mismatched handler/endpoint wiring is impossible or caught at compile time.

## Task E2: align extractor wrappers between api-core and api-axum

### Current gap

There are similarly named wrappers in `api-core` and `api-axum` (`Json`, `Path`, `Query`, `Body`, `Sse`). This invites accidental use of the wrong wrapper in signatures.

### Implementation

Choose one of these designs:

1. `api-core` owns the public wrapper types; `api-axum` implements Axum traits for those types.
2. `api-axum` re-exports core wrappers and adds framework impls through newtype adapter modules.

Recommended: `api-core` owns the types; adapters implement framework traits through feature-gated impls or adapter crates.

### Acceptance criteria

- Endpoint signatures use one canonical `Path<T>`, `Query<T>`, `Body<T>`, `Json<T>`, `Sse<T>`.
- The macro recognizes the canonical wrappers.
- Axum handlers compile without manual conversion wrappers.

---

# Track F: make `api-ls` a real gateway

## Task F1: replace the synchronous proxy loop with an async LSP gateway core

### Current gap

The gateway currently has a synchronous, single-loop JSON-RPC shape. A production gateway must concurrently read client messages and backend messages, map request IDs, forward notifications, merge diagnostics, and handle backend-initiated requests.

### Implementation

Use either:

- `lsp-server` plus crossbeam/tokio channels, or
- `tower-lsp` with internal backend clients.

Core architecture:

```text
client stdin/stdout
  <-> api-ls router
      <-> rust-analyzer backend client
      <-> effect-ts backend client
      <-> contract/symbol/usage index service
```

Implement:

- request-id translation per backend
- cancellation forwarding
- diagnostics fan-in
- progress/log message forwarding
- workspace folder changes
- didOpen/didChange/didClose synchronization to the right backend(s)
- generated virtual document handling

### Acceptance criteria

- `initialize` returns merged gateway capabilities.
- rust-analyzer diagnostics appear through `api-ls`.
- TypeScript/Effect diagnostics appear through `api-ls`.
- Requests in flight to both backends can complete out of order safely.
- The gateway does not deadlock when a backend emits notifications while a request is pending.

## Task F2: use the Effect-aware TypeScript backend by default

### Current gap

The default TypeScript backend command is `typescript-language-server --stdio`, with an optional language-service plugin string. The original decision is Effect v4 + its LSP paradigm, so the backend must be Effect-aware by default.

### Implementation

Support configured backend:

```json
{
  "typescript": {
    "command": "effect-tsgo",
    "args": ["--stdio"]
  }
}
```

or whatever executable name is chosen by the Effect v4 tooling. Keep it configurable, but ship the default docs and wrapper for the Effect-aware backend.

The generated TypeScript project must ensure the Effect language service can see generated files and project config.

### Acceptance criteria

- `api-ls doctor` verifies the Effect TS backend is installed and runnable.
- Effect diagnostics such as floating effects are visible through `api-ls`.
- The usage classifier can consume Effect diagnostics.

## Task F3: implement semantic cross-language lookup from symbol graph

### Implementation

For every `textDocument/definition`, `references`, `hover`, and `rename` request:

1. Determine whether the position is inside a Rust source file, TS source file, or generated TS file.
2. Ask the native backend for the local symbol when useful.
3. Resolve the symbol into the rust-ts symbol graph.
4. Return merged/rewritten locations.

### Required feature matrix

| Feature | Rust -> TS | TS -> Rust |
| --- | --- | --- |
| endpoint definition | yes | yes |
| endpoint references | yes | yes |
| DTO type definition | yes | yes |
| DTO type references | yes | yes |
| DTO field definition | yes | yes |
| DTO field references | yes | yes |
| error type definition | yes | yes |
| error variant / tag definition | yes | yes |
| error variant references | yes | yes |
| hover | yes | yes |
| prepareRename | yes | yes |
| rename | yes | yes |

### Acceptance criteria

- The user never lands in generated files unless explicitly opening generated output.
- Native Rust and native TS results are preserved and cross-language results are added.
- Generated TS file locations are rewritten to Rust locations for definition and hover.

## Task F4: implement wire-contract rename

### Current gap

The docs say field rename is wire-contract rename, but the implementation must do the coordinated edit.

### Semantics

Renaming TS field:

```ts
user.displayName -> user.fullName
```

must update the Rust API contract. Preferred edit:

```rust
pub display_name: String
```

becomes:

```rust
pub full_name: String
```

and all Rust references to `display_name` become `full_name` through rust-analyzer rename.

Serde handling rules:

1. If the enclosing container has `rename_all = "camelCase"` and the Rust rename `full_name` naturally maps to `fullName`, no explicit field `rename` is needed.
2. If the new TS name cannot be produced by the container rename rule, write or update `#[serde(rename = "...")]`.
3. If there are aliases for backward compatibility, preserve aliases only when explicitly configured.
4. Do not silently preserve the old wire key.

Endpoint rename:

- Rust `get_user` -> TS `getUser` by naming rule.
- Route path does not change unless the route literal is renamed separately.

Error tag rename:

- Rust variant rename updates `_tag` wire value unless an explicit Serde rename is present and the user chooses to preserve it.
- `catchTag("Old")` becomes `catchTag("New")`.

### Acceptance criteria

- Rename works from Rust or TypeScript entry point.
- Field rename updates Rust code, TS code, generated cache, and symbol graph.
- Conflicts are detected before edit generation.
- Generated cache is invalidated/regenerated after the edit.

## Task F5: replace string-based usage scanning with semantic Effect usage indexing

### Current gap

The scanner currently classifies usages by line text. That cannot support aliases, re-exports, imported function aliases, object destructuring, wrapper functions, TS project references, or real Effect program liveness.

### Implementation

Use TypeScript compiler services or the Effect-aware backend to resolve references semantically:

1. Resolve generated endpoint accessor symbols.
2. Ask TS for references to those symbols.
3. For each reference, inspect AST ancestors.
4. Merge Effect diagnostics such as floating Effect.
5. Classify usage.

Usage classes:

```text
strong:
  yielded in Effect.gen
  returned from an Effect-producing function
  piped/composed into Effect/Stream/Layer combinators
  passed into Effect.all / Layer.effect / Stream.fromEffect
  exported as a program that is itself strongly used
  run at an application boundary

weak:
  import-only
  reference-only
  assigned but never composed
  constructed and dropped

invalid:
  floating Effect diagnostic
  explicitly discarded with void
  inside unreachable code if TS can prove it

unknown:
  dynamic dispatch or alias escape
```

### Acceptance criteria

- Aliased imports are handled.
- Destructuring is handled.
- Re-exported programs are handled at least one hop.
- Floating endpoint Effects do not count as strong usage.
- The usage index records source locations, reason, diagnostics, and confidence.

---

# Track G: build and CI integration

## Task G1: make `api check` the single validation command

### Implementation

`api check` should run:

1. collect all configured contracts
2. generate hidden packages
3. generate symbol graph
4. run Rust `cargo check`
5. run generated TypeScript typecheck
6. run Effect diagnostics
7. build usage index
8. run unused endpoint policy
9. fail if stale files or diagnostics exist

Recommended commands:

```sh
cargo api check
cargo api gen
cargo api check-usages
cargo api doctor
```

### Acceptance criteria

- CI can run one command for the Rust/TS API integration.
- Stale generated packages are detected.
- Missing generated packages are generated or reported clearly.
- `--deny-unused-endpoints` fails CI on unused endpoints.

## Task G2: add repository CI

### Implementation

Add GitHub Actions:

```text
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm --prefix npm ci
npm --prefix npm test
cargo run -p api-collector --bin api -- doctor
integration fixture: collect -> gen -> npm typecheck -> check-usages -> build lint
```

### Acceptance criteria

- PRs cannot merge if generated Effect code does not typecheck.
- UI macro tests run in CI.
- Runtime tests run in CI.
- LSP protocol smoke tests run in CI.

## Task G3: improve build-lint source locations

### Current gap

The build bridge can warn/deny based on the usage index, but stable Cargo warnings are not enough for exact endpoint spans.

### Implementation

Keep three layers:

1. LSP diagnostics with exact source ranges.
2. Build-script warnings for stable Cargo visibility.
3. Deny mode through generated `compile_error!` for CI enforcement.

Add the endpoint source file/range to the generated diagnostic payload and include it in warning text. For native exact rustc-style spans later, add a separate lint-driver task.

### Acceptance criteria

- LSP unused endpoint diagnostic appears at the endpoint name.
- Cargo warning includes route, accessor, and source file.
- Deny mode fails CI with the same route/accessor/source metadata.

---

# Track H: npm and editor packaging

## Task H1: implement the npm language-server launcher

### Current gap

The npm `@rust-ts-integration/language-server` package exposes `./src/index.ts` as `api-ls`, but that file only exports a constant and is not an executable launcher.

### Implementation

Replace it with a real bin:

```ts
#!/usr/bin/env node
import { spawn } from "node:child_process"
import { fileURLToPath } from "node:url"

const binary = resolveBundledOrCargoInstalledApiLs()
const child = spawn(binary, process.argv.slice(2), {
  stdio: "inherit"
})
child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal)
  process.exit(code ?? 1)
})
```

Packaging choices:

- Bundle prebuilt binaries per platform.
- Or require `cargo install` and make the wrapper discover it.
- Or use `napi-rs` / `cargo-cp-artifact` for npm publishing.

### Acceptance criteria

- `npx api-ls --version` works.
- Editors can configure the npm package as the language server command.
- The wrapper forwards stdio unchanged.

## Task H2: add editor setup docs for gateway-only mode

### Implementation

Document Neovim, Zed, VS Code, Helix where possible, all using `api-ls` as the single gateway server for Rust+TS workspaces.

Make clear that users should not separately start rust-analyzer and TypeScript servers for the same workspace when gateway mode is active.

### Acceptance criteria

- There is one copy-paste config per editor.
- `api-ls doctor` detects duplicate server configurations where possible.

---

# Track I: end-to-end examples

## Task I1: upgrade `examples/monorepo-basic` to a full working app

### Current gap

The example prints a contract and references a neighboring TS app, but it does not prove the complete workflow.

### Implementation

Add:

```text
examples/monorepo-basic/
  Cargo.toml
  src/main.rs
  app/package.json
  app/tsconfig.json
  app/src/index.ts
  scripts/check.sh
```

`check.sh` should:

```sh
cargo run -p api-collector --bin api -- collect --package monorepo-basic --out target/api-contract/server-api.json
cargo run -p api-collector --bin api -- gen --contract target/api-contract/server-api.json
npm --prefix app ci
npm --prefix app run typecheck
cargo run -p api-collector --bin api -- check-usages --contract target/api-contract/server-api.json --out target/api-contract/graph/effect-usage-index.json --ts-dir app/src
```

### Acceptance criteria

- The example has an actual TS client program.
- The generated package is imported by TS.
- `yield* users.getUser(...)` counts as strong usage.
- Removing the TS usage causes an unused-endpoint diagnostic.

## Task I2: add an LSP protocol smoke fixture

### Implementation

Create a test harness that starts `api-ls`, opens the Rust and TS files from the example, and issues:

- initialize
- didOpen Rust
- didOpen TS
- definition on `users.getUser`
- references on Rust `get_user`
- hover on `users.getUser`
- prepareRename on `displayName`
- rename `displayName` -> `fullName`

### Acceptance criteria

- Returned locations point to expected files/ranges.
- Rename workspace edit includes Rust and TS edits.
- Generated files are not exposed as final definition targets.

---

# Recommended execution order

The tracks above are independently implementable, but this order reduces rework:

1. Add CI and generated-package typecheck fixture.
2. Implement transitive type/error collection.
3. Implement real `api collect` through a generated collector binary.
4. Upgrade IR to v2 and emit symbol graph with TS generated ranges.
5. Expand Serde lowering.
6. Fix generated Effect package semantics until the fixture typechecks.
7. Harden Effect runtime tests.
8. Make Axum integration route-safe and wrapper-consistent.
9. Replace string usage scanner with TS/Effect semantic scanner.
10. Rebuild `api-ls` as a production async gateway.
11. Implement cross-language definition/references/hover from symbol graph.
12. Implement wire-contract rename.
13. Wire unused endpoint diagnostics into LSP, build warnings, and deny mode.
14. Package the npm language-server launcher.
15. Upgrade examples and docs.

# Non-negotiable regression tests

Before calling the implementation complete, these tests should exist:

- `trybuild` tests for invalid endpoint return types.
- `trybuild` tests for unsupported public field types.
- `trybuild` tests for unsupported custom Serde serializers.
- Serde rename matrix tests.
- Generated Effect package typecheck test.
- Runtime unary HTTP tests.
- Runtime SSE tests.
- Collector integration test using a temporary Cargo workspace.
- Workspace graph test with two Rust API crates and one shared DTO crate.
- Usage-index test with aliased imports and floating Effects.
- LSP definition test TS -> Rust.
- LSP references test Rust -> TS.
- LSP field rename test TS -> Rust wire-contract rename.
- Build-lint test missing usage index.
- Build-lint test unused endpoint deny mode.

# Practical near-term milestone

The fastest meaningful next milestone is:

```text
A fresh example app can run:

cargo api check
npm typecheck

and then in an editor:

- TS definition on users.getUser jumps to Rust get_user
- Rust references on get_user show TS yield* users.getUser
- removing the TS usage reports api::unused_endpoint
```

That milestone requires only these tasks:

- A1 transitive collection
- A2 real collect command
- B2 symbol graph with TS ranges
- D1 generated package typecheck fixture
- F1 async gateway core
- F2 Effect-aware TS backend
- F3 semantic lookup for definition/references
- F5 semantic usage index, even if initially limited
- G1 `api check`
- H1 npm launcher

Everything else can follow, but without these tasks the project remains a promising prototype rather than the original cross-language API tool.
