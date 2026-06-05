# Follow-up implementation guide

This guide is the canonical plan for turning the current prototype into the intended Rust ↔ Effect TypeScript API tool.

The target product is a compiler-backed Rust API contract system where Rust endpoints, DTOs, errors, routes, and streams are the source of truth; the generated TypeScript surface is Effect-native; generated files are hidden cache artifacts; and a gateway-only language server provides bidirectional cross-language navigation, references, rename, hover, diagnostics, and field-level behavior.

## Locked product decisions

These decisions are fixed for the implementation plan.

- Rust is the source of truth for exported API endpoints, DTOs, domain errors, routes, streams, and wire names.
- Export starts from explicit API roots, not from every public Rust item.
- The TypeScript API is Effect-native: endpoint calls return `Effect.Effect<A, E, R>` and streams return `Stream.Stream<A, E, R>`.
- Rust `Result<T, E>` maps to the Effect error channel, not to a success-channel `Result` and not to Promise rejection.
- Effect Schema is generated for runtime encoding, decoding, validation, and decoded/encoded type extraction.
- Generated TypeScript files are disposable cache artifacts under `target/api-contract/...`; users should not edit or check them in.
- `api-ls` runs in gateway mode only. It is the single language-server entry point and delegates to rust-analyzer plus the Effect-aware TypeScript backend.
- There is no companion/fallback LSP mode in the final product.
- Field rename from either Rust or TypeScript is a wire-contract rename. It updates the Rust API contract and TS usages, not merely a local Serde compatibility shim.
- Unsupported public API shapes fail Rust compilation with actionable diagnostics.
- Exported endpoints with no live Effect usage are reported through editor diagnostics and can be warned/denied in build/CI.

## Definition of done

The system is complete when a fresh monorepo can author only Rust API code like this:

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

and then use the generated TypeScript API without editing generated files:

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
- `api-ls` is the only configured language server for the workspace and delegates to rust-analyzer plus the Effect-aware TypeScript backend.
- TS go-to-definition on `users.getUser`, `User`, `displayName`, and `catchTag("UserNotFound")` jumps to the Rust endpoint, type, field, or error variant.
- Rust find-references on an endpoint includes TS Effect call sites.
- Rust find-references on an error variant includes TS `catchTag` / `catchTags` handling sites.
- Rust find-references on a DTO field includes TS property reads and writes.
- Rename from either Rust or TS updates the wire contract and all relevant Rust/TS usages.
- Unused endpoint diagnostics count only strong/live Effect usages.
- Unsupported endpoint shapes fail rustc with precise, actionable diagnostics.

## Verified current state

The repository has the right shape and a useful prototype, but several pieces are still scaffolding or semantic approximations.

| Area | Current verified state | Main implication |
| --- | --- | --- |
| Workspace | The root Cargo workspace includes `api-core`, `api-ir`, `api-macros`, `api-collector`, `api-gen-effect-v4`, `api-ls`, `api-build`, `api-axum`, fixtures, and examples. | The planned crate split exists. Work should focus on semantics, not rearranging crates. |
| IR | `ApiContract` currently contains `package_name`, `endpoints`, `types`, and `errors`. `Field` stores `rust_name`, `wire_name`, `ts_name`, and a Rust-ish `SourceRange`. | IR v1 is enough for prototypes but not enough for exact Effect channels, TS ranges, source maps, or wire rename. |
| Core traits | `ApiType`, `ApiError`, endpoint wrappers, `ApiModule`, and primitive/external mappings exist. | The core API can be extended; it does not yet support automatic transitive registration. |
| Macros | `ApiType`, `ApiError`, and `#[api]` exist. The Serde parser handles a small subset: rename, rename_all, and narrow `skip_serializing_if = "...Option::is_none"`. | The macro layer needs richer Serde/wire semantics and precise source spans. |
| CLI | `api gen`, `api check`, and `api check-usages` have useful behavior. `api collect` currently writes an empty contract. `api watch` regenerates once and exits. | The real Rust-source-to-TS loop is not operational until collection is compiler-backed and watch is persistent. |
| Collector | `collect_contract` requires manual `types` and `errors` lists. The usage scanner is line/string based. | Low-code transitive export and reliable Effect usage detection are not implemented yet. |
| Effect generator | The generator emits `schemas.ts`, `errors.ts`, `endpoints.ts`, `layer.ts`, `index.ts`, `package.json`, and `tsconfig.paths.json`. | The generated package shape is in place, but it needs typechecked fixtures, encoded/decoded correctness, and symbol range tracking. |
| Type mapping | `api-core` declares an integer policy where `i64`, `u64`, `usize`, and `isize` should be string encoded, but the TS generator maps `i64` to `number` / `Schema.Number`. | This is a real wire-safety bug. It must be fixed before API correctness claims are made. |
| External types | `Uuid`, `DateTime<Utc>`, `Decimal`, and `JsonValue` have external mappings. The generator currently renders externals with `Schema.declare(... => true)`. | Runtime validation for important external wire types is missing. |
| LSP | `api-ls` starts rust-analyzer and a TypeScript language server, handles several LSP methods, and consumes `rust-ts-symbols.json` plus `effect-usage-index.json`. | It is gateway-shaped but not production-grade. It needs async proxying, hard backend requirements, semantic lookup, graph generation, and complete feature coverage. |
| Symbol graph | `api-ls` and `api-build` consume `target/api-contract/rust-ts-symbols.json`. The repository has fixtures/docs for this shape, but no real producer was found. | Cross-language definition/references/rename/hover cannot be relied on until the generator writes a real graph. |
| Build lint bridge | `api-build` reads the usage index and symbol graph, emits Cargo warnings, and can write deny-mode `compile_error!` glue. | The bridge is useful, but it depends on missing/weak inputs and needs first-class `api check` integration. |
| Axum adapter | Axum wrappers, response conversion, domain error serialization, and SSE serialization exist. | The adapter proves feasibility but needs tighter route/metadata coupling and end-to-end contract tests. |
| npm runtime | The Effect runtime package contains fetch/SSE helpers and API client errors. | Runtime functionality exists but must be compiled/tested against generated packages and pinned Effect versions. |
| npm LSP wrapper | The wrapper currently exports a constant and is not an executable launcher. | Installation and editor integration are not yet real. |

## Critical gaps

### 1. Contract collection is not automatic or compiler-backed

The original vision requires Rust to be the source of truth. The current CLI `api collect` does not inspect a real crate root; it writes an empty contract. The realistic example compensates by manually passing `types` and `errors` to `CollectorInput`.

This prevents the core workflow:

```text
Rust API root -> collected contract -> hidden Effect package -> LSP graph -> TS usage index -> unused endpoint diagnostics
```

### 2. Transitive export is manual

Endpoint DTOs and errors should be exported because they are reachable from exported endpoints. The current collector cannot discover transitive types without being handed `types` and `errors` lists.

### 3. Source mapping and symbol identity are incomplete

The IR stores some source ranges, but most macro-generated ranges are call-site-ish placeholders. The IR does not store generated TS ranges. The LSP expects a symbol graph, but no production code writes it.

### 4. Serde compatibility is shallow

The macro parser does not model enough Serde behavior to claim wire compatibility. Missing or incomplete cases include internally/adjacently/externally tagged enum modes, untagged enums, flatten, default, skip, alias, transparent, deny_unknown_fields, with/serialize_with/deserialize_with, borrowed fields, and variant-specific rename_all.

### 5. Type mappings are not yet safe enough for APIs

The `i64` mismatch is the clearest issue: the core policy says string-encoded, but the generator emits `number`. External types use permissive declarations instead of real schemas/transforms. Option/nullability behavior also needs to distinguish required nullable, optional omitted, and optional nullable.

### 6. Generated Effect output is plausible but not proven

The generator emits TypeScript that looks Effect-shaped, but there is no generated-package typecheck fixture proving that generated schemas, errors, endpoint accessors, Layer, runtime helpers, and consumer code compile together.

### 7. Usage indexing is text-based

The current scanner cannot reliably resolve imports, aliases, reexports, destructuring, endpoint wrappers, TS type-only usage, live Effect composition, or Effect language-service diagnostics. It will produce false positives and false negatives for unused endpoint analysis.

### 8. Gateway LSP is structurally present but not production-grade

The current server starts backends and intercepts methods, but it should become a strict gateway: backend startup must be required, request/notification forwarding must be asynchronous and robust, generated files must be served consistently, and all cross-language behavior must use semantic graph lookup.

### 9. Wire-contract rename is not implemented

The current graph-driven rename applies the same replacement text to renamable locations. Final behavior needs semantic edits: rename Rust identifiers, rename route params, update or remove Serde attributes, update TS usages, regenerate cache, and preserve valid casing across Rust, wire, and TS names.

### 10. Multi-crate / multi-package behavior is not implemented

The final system should mirror Cargo package relationships into generated TS package relationships. The current contract has one `package_name` and does not model workspace dependencies or shared API crates deeply enough.

## Implementation tracks

The tasks below are grouped by layer. Tasks inside a track can often be implemented independently after the track prerequisites are met. Each task has explicit acceptance criteria so progress can be reviewed without relying on intent.

---

# Track A: compiler-backed contract collection

## A1. Add transitive registration to `ApiType` and `ApiError`

### Goal

Make endpoint reachability drive exported DTO and error discovery.

### Implementation

Add registries to `api-core`:

```rust
pub struct TypeRegistry { /* ordered map by SymbolId */ }
pub struct ErrorRegistry { /* ordered map by SymbolId plus TypeRegistry */ }
pub struct ContractRegistry { /* endpoints, types, errors */ }
```

Extend traits:

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

    fn register_error(registry: &mut ContractRegistry) {
        registry.insert_error(Self::error_def());
        Self::register_types(registry.types_mut());
    }
}
```

Generate recursive registration in derives:

```rust
fn register_types(registry: &mut TypeRegistry) {
    if registry.insert(Self::type_def()) {
        <Field1 as ApiType>::register_types(registry);
        <Field2 as ApiType>::register_types(registry);
    }
}
```

Implement recursive registration for wrappers and containers:

```rust
impl<T: ApiType> ApiType for Option<T> { /* register T */ }
impl<T: ApiType> ApiType for Vec<T> { /* register T */ }
impl<T: ApiType> ApiType for BTreeMap<String, T> { /* register T */ }
impl<T: ApiType> ApiType for Json<T> { /* delegate */ }
impl<T: ApiType> ApiType for Created<T> { /* delegate */ }
impl<T: ApiType> ApiType for Path<T> { /* delegate */ }
impl<T: ApiType> ApiType for Query<T> { /* delegate */ }
impl<T: ApiType> ApiType for Body<T> { /* delegate */ }
```

### Acceptance criteria

- Examples no longer manually pass `types: vec![...]` or `errors: vec![...]`.
- Request body, path params, query params, response body, stream item, and error payload types are exported transitively.
- Repeated shared types appear once in the final contract.
- Recursive/cyclic type graphs do not loop forever.

## A2. Replace endpoint metadata functions with endpoint descriptors

### Goal

Allow each endpoint to return both static metadata and a typed registration function.

### Implementation

Add:

```rust
pub struct EndpointDescriptor {
    pub endpoint: Endpoint,
    pub register: fn(&mut ContractRegistry),
}
```

Change `#[api]` expansion so generated endpoint metadata returns an `EndpointDescriptor` rather than a plain `Endpoint`. For a function:

```rust
#[api(method = "GET", path = "/users/{id}")]
async fn get_user(id: Path<UserId>) -> Result<Json<User>, GetUserError> { ... }
```

generate a descriptor whose `register` does:

```rust
<UserId as ApiType>::register_types(registry.types_mut());
<User as ApiType>::register_types(registry.types_mut());
<GetUserError as ApiError>::register_error(registry);
```

`ApiModule` should store descriptors and continue to expose endpoint IRs.

### Acceptance criteria

- `api_module!(..., endpoints = [get_user])` works without manually referencing hidden `__api_endpoint_get_user` functions.
- Hidden metadata names remain available only as implementation detail or compatibility shim.
- Endpoint registration covers nested wrapper return types and nested `Result` wrappers.

## A3. Implement real `api collect`

### Goal

Make the CLI collect real Rust API contracts from workspace crates.

### Implementation

Implement a compiler-backed collector command:

```sh
cargo api collect --package server --root server::api --out target/api-contract/server-api.json
```

Required steps:

1. Read Cargo metadata.
2. Resolve the package, manifest path, target directory, features, and API root.
3. Generate a temporary collector crate under `target/api-contract/collector/<package>/`.
4. Depend on the target package by path with matching features.
5. Compile and run the collector binary through Cargo.
6. Call the configured root function, collect the contract, and print JSON.
7. Write the contract, generated package metadata, and initial symbol graph seed.

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

### Acceptance criteria

- The default `api collect` path never writes an empty contract.
- `--empty` may exist only as an explicit debug/test flag.
- Missing root function produces a clear compiler or CLI diagnostic.
- Wrong root return type produces a clear diagnostic.
- Feature flags and target package dependencies match normal Cargo compilation.

## A4. Read package metadata for API configuration

### Goal

Remove repetitive CLI configuration.

### Implementation

Support:

```toml
[package.metadata.rust_ts]
ts_package = "@workspace/server-api"
api_root = "server::api"
features = []
```

Fallback:

```text
Cargo package `server-api` -> generated TS package `@generated/server-api`
```

### Acceptance criteria

- `cargo api collect --package server` works when metadata exists.
- Multiple API-enabled packages can be discovered from the workspace.
- CLI flags override metadata.

## A5. Build a workspace contract graph

### Goal

Support multiple Rust crates and generated TS packages.

### Implementation

Write:

```text
target/api-contract/workspace-contract.json
```

with package contracts and dependency edges:

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

- `api gen --workspace` generates all API packages in dependency order.
- Shared API crates are imported, not duplicated, when used by multiple API packages.
- TS path mappings include every generated package.

---

# Track B: IR v2 and symbol graph

## B1. Introduce versioned IR v2

### Goal

Represent the full API contract needed by Effect, LSP, rename, and build diagnostics.

### Implementation

Add schema versioning:

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
```

Upgrade endpoint success shape:

```rust
pub enum SuccessShape {
    Empty { status: u16 },
    Json { status: u16, ty: TypeRef },
    Binary { status: u16, media_type: Option<String> },
    Stream { item: TypeRef },
}
```

Preserve compatibility by reading v1 and converting to v2 during an interim period.

### Acceptance criteria

- `Json<T>` maps to JSON 200.
- `Created<T>` maps to JSON 201.
- `NoContent` maps to status 204 and TS `void` / `undefined` success.
- `Sse<T>` maps to a stream success channel.
- Binary upload/download can be represented without another breaking IR change.

## B2. Generate a real symbol graph

### Goal

Produce the file consumed by `api-ls` and `api-build`.

### Implementation

Write:

```text
target/api-contract/graph/rust-ts-symbols.json
```

for every endpoint, type, field, enum variant, error variant, error tag, route path param, and generated TS accessor.

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
          "generated": true
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

### Acceptance criteria

- `api gen` writes `rust-ts-symbols.json` every time it writes generated TS.
- `api-ls` no longer requires a hand-authored graph.
- The graph includes both generated TS locations and discovered user TS usage locations.
- The graph is deterministic across runs.

## B3. Replace placeholder Rust ranges with precise source ranges

### Goal

Make field-level navigation and rename reliable.

### Implementation

Do not rely on `file!()`, `line!()`, and `column!()` for field ranges. Instead:

1. During collection, map Rust module paths to source files.
2. Parse source files with `syn` or rust-analyzer-derived syntax information.
3. Match collected symbols to items by module path and item/field/variant name.
4. Compute zero-based UTF-16 LSP ranges.
5. Store both `nameRange` and `fullRange`.

### Acceptance criteria

- TS definition on a field lands on the Rust field identifier, not the struct or macro call.
- TS definition on an error tag lands on the Rust enum variant identifier.
- Rust rename on a field uses the field identifier range only.
- Multi-byte characters and Windows paths are handled correctly.

## B4. Add a tracked writer for generated TS ranges

### Goal

Record TS ranges as files are generated.

### Implementation

Replace string concatenation in `api-gen-effect-v4` with a small writer:

```rust
pub struct TrackedWriter {
    text: String,
    line: u32,
    character_utf16: u32,
    marks: Vec<GeneratedMark>,
}
```

Use it to mark:

- schema identifiers,
- type aliases,
- fields,
- endpoint namespaces,
- endpoint accessor names,
- route metadata,
- error classes,
- error tags,
- status metadata,
- service methods.

### Acceptance criteria

- Generated TS ranges are present in the symbol graph.
- Ranges survive formatting because generation owns formatting.
- The graph can redirect every generated declaration back to Rust.

---

# Track C: macros, Serde, and compile-time validation

## C1. Upgrade Serde interpretation

### Goal

Make generated wire shapes match Serde for API-relevant cases.

### Implementation

Prefer using `serde_derive_internals` or a dedicated Serde-attribute parser rather than expanding one-off parsing manually.

Support at least:

- container and field `rename`, `rename_all`, and variant rename rules,
- `tag`, `content`, and `untagged`,
- externally tagged, internally tagged, adjacently tagged, and untagged enum lowering,
- `flatten`,
- `default`,
- `skip`, `skip_serializing`, `skip_deserializing`,
- `skip_serializing_if`, especially `Option::is_none`,
- `transparent`,
- `alias` for decoding metadata,
- `deny_unknown_fields` as schema metadata.

For unsupported custom serializers, fail unless the field has an explicit API override.

### Acceptance criteria

- Serde enum tagging integration tests compare Rust JSON output with generated schema decode expectations.
- Flattened structs generate correct intersection/object schema behavior or fail with a precise diagnostic if unsupported.
- `skip` fields do not appear in generated TS.
- Defaults and optional fields match request/response semantics.

## C2. Fix optionality and nullability

### Goal

Represent missing and null separately.

### Implementation

Use four cases:

```rust
pub enum Optionality {
    Required,
    Optional,
    Nullable,
    OptionalNullable,
}
```

Rules:

- Plain `Option<T>` in a response is `T | null` unless omitted by Serde.
- `Option<T>` with `skip_serializing_if = "Option::is_none"` is optional on encoded output.
- Request DTOs with `default` can be optional on decode.
- Non-Option fields with `default` can be optional on decode but required in decoded type, depending on schema transform.

### Acceptance criteria

- Generated Schema distinguishes `Schema.NullOr(T)` from `Schema.optionalKey(T)`.
- Request decode accepts missing defaulted fields when Rust would.
- Response encode/decode matches Serde omission rules.

## C3. Require typed public errors

### Goal

Keep Effect error channels finite and useful.

### Implementation

Reject public endpoint errors such as:

```rust
anyhow::Error
Box<dyn std::error::Error>
String
std::io::Error
```

unless explicitly mapped by an API error adapter.

Accepted:

```rust
Result<Json<User>, GetUserError>
```

where `GetUserError: ApiError`.

### Acceptance criteria

- Compile-fail tests cover unsupported public error types.
- Diagnostics suggest deriving `ApiError` or mapping internal errors into a domain enum.
- Shared error enums and endpoint-specific error enums both work.

## C4. Validate route, path, query, and body signatures

### Goal

Catch API shape bugs at Rust compile time.

### Implementation

Extend `#[api]` validation:

- every route parameter has a matching `Path<T>` argument,
- every `Path<T>` argument appears in the route,
- no duplicate route params,
- at most one body extractor,
- GET/DELETE body policy is explicit,
- query extractors are named DTOs or well-defined field sets,
- unsupported extractors fail with help text,
- SSE endpoints return stream-compatible wrappers.

### Acceptance criteria

- Compile-fail tests cover missing path param, extra path param, duplicate body, unsupported extractor, invalid SSE return, and invalid method/body combinations.

## C5. Add precise unsupported-shape diagnostics

### Goal

Make unsupported public API shapes fail rustc at the right source location.

### Implementation

For derives and endpoint macros, emit `compile_error!` or trait obligations with spans attached to the offending field/type/return value.

Examples that must fail:

- tuple enum variants unless explicitly supported,
- trait objects,
- generic fields without `ApiType` bounds,
- arbitrary `Result<T, E>` inside DTOs,
- custom serializer without override,
- non-string map keys for JSON endpoints,
- raw futures/streams without a transport wrapper.

### Acceptance criteria

- trybuild tests assert the error text and approximate span.
- Diagnostics include a suggested fix or escape hatch where one exists.

---

# Track D: Effect v4 generator and runtime correctness

## D1. Fix primitive and integer wire mappings

### Goal

Make TypeScript wire types safe.

### Implementation

Align `api-core` policy and TS generation:

- `i8`, `i16`, `i32`, `u8`, `u16`, `u32`, `f32`, `f64` -> `number` where allowed.
- `i64`, `u64`, `i128`, `u128`, `usize`, `isize` -> string-encoded schema by default.
- Allow an explicit opt-in for unsafe JS numbers.

### Acceptance criteria

- `i64` no longer renders as `Schema.Number` by default.
- Generated request encoding converts decoded branded/string-safe values to wire form.
- Generated response decoding rejects unsafe integer values when configured as strings.

## D2. Replace permissive external schemas with real schemas/transforms

### Goal

Validate and transform important external types.

### Implementation

Generate runtime schemas for:

- `uuid::Uuid` as branded string with UUID validation,
- `chrono::DateTime<Utc>` as ISO string decoded to the configured TS representation,
- `rust_decimal::Decimal` as decimal string or Effect BigDecimal-backed decoded type,
- `serde_json::Value` as recursive JSON value schema,
- bytes in JSON as base64 string,
- bytes in binary endpoints as `Uint8Array` / `Blob` / stream types.

### Acceptance criteria

- `Schema.declare(... => true)` is not used for standard external API types.
- Invalid UUID/date/decimal payloads fail decoding.
- Encoded and decoded TS types differ where appropriate.

## D3. Generate request encoders from schemas

### Goal

Stop passing decoded values directly as JSON when the encoded shape differs.

### Implementation

For every endpoint, generate argument schemas with encoded and decoded forms:

```ts
export const GetUserArgs = Schema.Struct({ id: UserId })
export type GetUserArgs = Schema.Schema.Type<typeof GetUserArgs>
export type GetUserArgsEncoded = Schema.Schema.Encoded<typeof GetUserArgs>
```

Endpoint runtime should call schema encoders for path, query, and body values before building the request.

### Acceptance criteria

- Branded/newtype/Date/Decimal values encode correctly.
- Path and query parameters use encoded wire values.
- Body values are encoded before JSON serialization.

## D4. Add generated-package typecheck fixtures

### Goal

Prove generated Effect code compiles.

### Implementation

Create fixtures that:

1. build a realistic `ApiContract`,
2. render the hidden package into a temp directory,
3. create a temp TS consumer project,
4. install/link the local runtime package,
5. run `tsc --noEmit`,
6. optionally run Effect language-service diagnostics.

### Acceptance criteria

- CI typechecks generated packages for unary endpoints, errors, SSE streams, newtypes, external types, optional fields, and multiple namespaces.
- Generated package imports resolve through `tsconfig.paths`.
- `Effect.catchTag` works on generated domain errors.

## D5. Harden generated errors and status decoding

### Goal

Make error decoding exact and helpful.

### Implementation

Generate error schemas keyed by status and tag. The runtime should:

1. select candidate schemas by HTTP status,
2. decode by `_tag` when multiple variants share a status,
3. return `UnexpectedStatusError` when no declared status matches,
4. return `DecodeError` when a declared status body does not match the declared error schema.

### Acceptance criteria

- Two error variants can share a status if their tags differ.
- Unknown status is `UnexpectedStatusError`.
- Known status with invalid body is `DecodeError`.
- `catchTag` type narrowing works in consumer fixtures.

## D6. Harden SSE runtime protocol

### Goal

Make stream errors and decoding reliable.

### Implementation

Specify the SSE wire protocol:

- normal event: `event: message`, JSON data payload,
- domain error event: `event: api-error`, data `{ status, body }`,
- optional heartbeat/comment handling,
- malformed frame -> `RemoteProtocolError`,
- declared error frame -> fail stream with domain error,
- response decode failure -> `DecodeError`.

### Acceptance criteria

- Runtime tests cover normal events, domain error events, malformed JSON, unknown status, cancellation, and stream close.
- Generated stream accessors return `Stream.Stream<Item, DomainError | ApiClientError, ServerApi>`.

## D7. Keep HttpApi/RPC metadata optional

### Goal

Support Effect ecosystem integration without coupling public API stability to unstable backend details.

### Implementation

Public generated surface remains:

```ts
users.getUser(args): Effect.Effect<User, GetUserError | ApiClientError, ServerApi>
```

Add optional backend emitters:

```text
effect_http_api.ts
effect_rpc.ts
```

behind config.

### Acceptance criteria

- The default consumer does not need to import HttpApi/RPC metadata.
- Metadata generation can be disabled without changing public endpoint accessors.
- Metadata fixtures typecheck when enabled.

---

# Track E: semantic TypeScript and Effect usage indexing

## E1. Replace line scanning with TypeScript semantic analysis

### Goal

Detect actual endpoint references and live Effect usage.

### Implementation

Use the TypeScript compiler API or the same TS backend used by `api-ls` to:

1. load the user TS project,
2. include generated package path mappings,
3. resolve each generated endpoint accessor symbol,
4. find references through TypeScript symbol resolution,
5. map references back to endpoint IDs.

### Acceptance criteria

- Aliased imports work.
- Reexports work.
- Destructuring works.
- Type-only imports do not count as runtime usage.
- References to unrelated objects with the same property names do not count.

## E2. Integrate Effect language-service diagnostics

### Goal

Use Effect’s own understanding of live/floating Effects.

### Implementation

Run the Effect language service in the TS project and ingest diagnostics, especially floating Effect diagnostics. Usage classification should consider both AST shape and Effect diagnostics.

Classification:

```text
Strong:
  yielded, returned, piped/composed, used in Layer/Service construction, exported as a live program, or run at an application boundary

Weak:
  imported, referenced as a value, assigned locally without evidence of composition, type-only mention

Invalid:
  floating Effect call, explicitly discarded Effect, Effect diagnostic marks it as not used correctly

Unknown:
  dynamic dispatch or escaped function value where liveness cannot be proven
```

### Acceptance criteria

- `users.getUser({ id })` by itself is invalid/weak, not strong.
- `yield* users.getUser({ id })` is strong.
- `return users.getUser({ id })` is strong.
- `users.getUser({ id }).pipe(...)` is strong.
- Effect diagnostics are stored in `effect-usage-index.json` with ranges.

## E3. Index error handling and field usage

### Goal

Support references beyond endpoints.

### Implementation

Index:

- `Effect.catchTag("UserNotFound", ...)`,
- `Effect.catchTags({ UserNotFound: ... })`,
- generated error classes,
- DTO property reads/writes,
- endpoint argument fields,
- route constants if exposed.

### Acceptance criteria

- Rust find-references on an error variant includes TS `catchTag` and `catchTags` handlers.
- Rust find-references on a DTO field includes TS property reads/writes for that field only.
- Property references are type-aware, not text search.

## E4. Persist a rich usage index

### Goal

Provide stable data to LSP and build lints.

### Implementation

Write:

```text
target/api-contract/graph/effect-usage-index.json
```

with:

```json
{
  "schemaVersion": 2,
  "packageName": "@workspace/server-api",
  "generatedAt": "...",
  "contractHash": "...",
  "tsProgramHash": "...",
  "endpoints": [
    {
      "endpointId": "...",
      "accessorPath": ["users", "getUser"],
      "strong": 2,
      "weak": 0,
      "invalid": 1,
      "unknown": 0
    }
  ],
  "usages": []
}
```

### Acceptance criteria

- Index staleness can be detected without relying only on file modification times.
- Build lints can distinguish missing, stale, and valid indexes.
- LSP can incrementally update diagnostics after TS edits.

---

# Track F: gateway-only LSP

## F1. Make backend startup mandatory

### Goal

Honor gateway-only mode.

### Implementation

During `initialize`, `api-ls` must fail initialization if rust-analyzer or the configured Effect-aware TS backend cannot start or initialize.

No soft fallback. No “backend unavailable but keep going.”

### Acceptance criteria

- Missing rust-analyzer fails `initialize` with a clear message.
- Missing TS/Effect backend fails `initialize` with a clear message.
- Error messages include the configured command and remediation hint.

## F2. Replace synchronous proxying with an async JSON-RPC core

### Goal

Make gateway mode reliable under real editor load.

### Implementation

Use an async runtime and maintain:

- client request ID -> backend request ID maps,
- backend request ID -> client request ID maps,
- notification forwarding queues,
- cancellation forwarding,
- backend stderr logging,
- graceful restart or fail-fast policy,
- request timeouts for internal operations.

### Acceptance criteria

- Concurrent Rust and TS LSP requests do not block each other.
- Backend notifications are forwarded promptly.
- `$/cancelRequest` is forwarded.
- Shutdown/exit cleans up both backends.

## F3. Merge and advertise backend capabilities correctly

### Goal

The editor should see the union of rust-analyzer, Effect TS backend, and cross-language capabilities.

### Implementation

On initialize:

1. initialize rust-analyzer,
2. initialize TypeScript/Effect backend,
3. inspect both capability objects,
4. advertise merged capabilities plus cross-language overrides.

### Acceptance criteria

- Formatting, completion, semantic tokens, code actions, call hierarchy, and workspace symbols work where backends support them.
- Cross-language definition/references/rename/hover are handled by `api-ls` and merged with backend results.

## F4. Serve generated files through a virtual/generated-file layer

### Goal

Make generated files available to TypeScript while keeping navigation transparent.

### Implementation

`api-ls` should:

- ensure generated packages exist before initializing TS backend,
- expose generated files to TS via real cache paths and path mappings,
- redirect user navigation away from generated declarations to Rust source,
- optionally provide a command to open generated source explicitly.

### Acceptance criteria

- TS backend resolves generated package imports.
- User go-to-definition does not land in generated files by default.
- Generated files can still be inspected when explicitly requested.

## F5. Implement semantic cross-language definition and references

### Goal

Make navigation work without hand-authored graph locations.

### Implementation

Use the symbol graph plus backend reference results:

- TS generated symbol -> Rust source location,
- TS user usage -> generated symbol -> Rust source location,
- Rust symbol -> TS generated symbol -> TS semantic references,
- Rust error variant -> TS `catchTag` references,
- Rust field -> TS typed property references.

### Acceptance criteria

- Definition/references work for endpoints, DTOs, fields, error variants, error tags, and endpoint argument fields.
- Results are deduplicated and include backend results.
- Generated locations are hidden unless explicitly requested.

## F6. Implement wire-contract rename

### Goal

Rename from either language changes the public API contract intentionally.

### Implementation

Rename rules:

- TS field rename `displayName -> fullName` updates Rust field `display_name -> full_name` when possible.
- If a container `rename_all` rule maps the new Rust name to the new wire name, do not add a redundant `serde(rename)`.
- If no container rule can express the new wire name, add or update `#[serde(rename = "fullName")]`.
- Rust field rename updates TS field usages and wire name according to the rename policy.
- Route param rename updates route path, handler argument, request args, and TS usages.
- Endpoint function rename updates TS accessor and TS usages, but does not change HTTP method/path unless the route symbol is renamed explicitly.
- Error variant rename updates `_tag`, generated error class, and TS `catchTag` / `catchTags` usages.

### Acceptance criteria

- Rename has conflict detection for sibling fields, endpoint names, error tags, and route params.
- Rename produces workspace edits across Rust source and TS user files.
- Generated files are not edited directly; they are regenerated.
- Prepare-rename rejects unsafe positions with a helpful reason.

## F7. Publish diagnostics, CodeLens, and hover metadata

### Goal

Expose the API state directly in editors.

### Implementation

Diagnostics:

- unused endpoint,
- stale generated package,
- stale usage index,
- unresolved generated package,
- unsupported type fallback accidentally emitted,
- TS usage of removed endpoint,
- mismatched route/path params.

Hover:

- route,
- Rust path,
- TS accessor path,
- Effect signature,
- domain errors/statuses,
- usage count.

CodeLens:

- “N TS usages”,
- “Open generated signature”,
- “Run API check”,
- “Endpoint appears unused”.

### Acceptance criteria

- Diagnostics update after Rust or TS edits.
- Hover includes backend hover plus API metadata.
- CodeLens commands work through LSP commands.

---

# Track G: build, CI, and command workflow

## G1. Implement `cargo api check`

### Goal

Make one command validate the whole system.

### Implementation

`cargo api check` should run:

1. collect workspace contracts,
2. generate hidden packages,
3. typecheck generated packages and consumer fixtures,
4. run TypeScript/Effect diagnostics,
5. update semantic usage index,
6. validate symbol graph freshness,
7. run usage lints according to policy,
8. optionally run `cargo check` with generated lint glue enabled.

### Acceptance criteria

- Fresh repo example passes with one command.
- Stale generated output fails with actionable instructions.
- Missing TypeScript dependency/setup fails with actionable instructions.
- `--deny-unused-endpoints` fails when an exported endpoint has no strong TS usage.

## G2. Strengthen build lint bridge

### Goal

Make Rust-side unused endpoint feedback reliable.

### Implementation

Improve `api-build` to:

- use contract/index hashes rather than modification times only,
- distinguish missing graph, missing usage index, stale usage index, and zero usage,
- print Cargo warnings in warn mode,
- emit `compile_error!` only in deny mode,
- include endpoint route/accessor in every diagnostic,
- document build.rs setup.

### Acceptance criteria

- Warn mode never fails builds.
- Deny mode fails builds only for real lint errors or explicitly configured stale/missing inputs.
- Diagnostics point users to the exact command that regenerates inputs.

## G3. Add CI workflows

### Goal

Prevent regressions across Rust, TS, generated packages, and examples.

### Implementation

Add GitHub Actions for:

- `cargo fmt --check`,
- `cargo clippy --workspace --all-targets -- -D warnings` after resolving current lint policy,
- `cargo test --workspace`,
- trybuild tests,
- npm install/typecheck for runtime and wrapper packages,
- generated-package typecheck fixtures,
- example `cargo api check`.

### Acceptance criteria

- CI exercises the complete Rust -> generated Effect -> TS consumer loop.
- CI fails if generated TS no longer typechecks.
- CI fails if core navigation symbol graph fixtures become stale.

---

# Track H: Axum and transport integration

## H1. Couple route registration to endpoint descriptors

### Goal

Avoid divergence between API metadata and actual Axum routes.

### Implementation

Provide a route helper:

```rust
ApiRouter::new(api())
    .api_route(get_user, get_user_handler)
    .api_route(create_user, create_user_handler)
```

or a macro:

```rust
api_routes! {
    get_user => get(get_user_handler),
    create_user => post(create_user_handler),
}
```

The helper should verify method/path from the descriptor and register normalized Axum paths.

### Acceptance criteria

- A route cannot accidentally register a handler under a path different from the exported API path without an explicit override.
- Tests compare collected routes to Axum router behavior.

## H2. Align framework wrappers with core wrappers

### Goal

Avoid confusion between `api_core::Json` and `api_axum::Json`.

### Implementation

Choose one policy:

- Re-export framework wrappers from the adapter with clear names, or
- make adapter wrappers implement/convert from core wrappers, or
- collapse shared wrapper definitions into core and add framework trait impls under adapter features.

### Acceptance criteria

- User examples do not need to mentally track two unrelated `Json`, `Path`, `Query`, `Body`, or `Sse` types.
- `#[api]` recognizes the actual wrappers used by Axum handlers.

## H3. Expand transport coverage after unary + SSE are solid

### Goal

Support expected API-layer transports without destabilizing MVP.

### Implementation order:

1. Unary JSON HTTP.
2. SSE server streams.
3. Binary download.
4. Binary upload.
5. Multipart.
6. WebSocket duplex.

### Acceptance criteria

- Each transport has Rust adapter tests, generated TS typecheck tests, and runtime tests.
- Unsupported transport shapes fail at compile time.

---

# Track I: npm packaging and editor installation

## I1. Make the language-server wrapper executable

### Goal

Allow editors and package managers to run `api-ls` consistently.

### Implementation

Create an npm package with:

```json
{
  "name": "@rust-ts-integration/language-server",
  "bin": {
    "api-ls": "./dist/index.js"
  }
}
```

The wrapper should:

- find a local `api-ls` binary,
- optionally download/use a platform binary in published releases,
- forward stdio untouched,
- print clear install diagnostics when missing.

### Acceptance criteria

- `npx api-ls` or package-manager equivalent starts the gateway.
- Editor config can point at the npm binary.
- Wrapper tests verify argument forwarding and failure messages.

## I2. Add root package-manager files

### Goal

Make TS runtime and generated-package tests reproducible.

### Implementation

Add root-level package-manager configuration:

- `package.json`,
- lockfile,
- workspace config (`pnpm-workspace.yaml`, npm workspaces, or chosen manager),
- scripts for runtime typecheck, generated fixture typecheck, and Effect diagnostics.

### Acceptance criteria

- A fresh checkout can run documented TS commands without guessing package-manager setup.
- CI uses the same commands as developers.

## I3. Document editor setup for gateway-only mode

### Goal

Prevent users from accidentally running duplicate Rust/TS language servers.

### Implementation

Document:

- how to register `api-ls` as the only server for Rust and TypeScript in supported editors,
- how `api-ls` starts rust-analyzer and the TS/Effect backend,
- how generated package paths are wired,
- how to diagnose backend startup failures,
- how to opt into build-time Effect diagnostics.

### Acceptance criteria

- Docs explicitly say not to run rust-analyzer or TypeScript LSP separately for workspaces using this tool.
- At least one Neovim, one VS Code-compatible, and one generic LSP example is present.

---

# Track J: end-to-end examples and tests

## J1. Replace toy examples with runnable end-to-end examples

### Goal

Prove the complete product loop.

### Implementation

Create examples:

```text
examples/e2e-axum-effect/
  server/
  app/
  README.md
```

The example should include:

- Rust DTOs and errors,
- unary endpoint,
- SSE endpoint,
- generated hidden package,
- TS Effect consumer,
- `catchTag`,
- field usage,
- endpoint usage,
- unused endpoint test,
- LSP setup sample.

### Acceptance criteria

- `cargo api check` passes.
- Generated TS typechecks.
- Axum server responds with JSON and SSE matching generated schemas.
- Removing the TS usage causes unused endpoint diagnostics.

## J2. Add golden contract and generated-package tests

### Goal

Make behavior reviewable.

### Implementation

For key fixtures, store expected outputs:

```text
tests/golden/basic.contract.json
tests/golden/basic.schemas.ts
tests/golden/basic.errors.ts
tests/golden/basic.endpoints.ts
tests/golden/basic.symbols.json
```

### Acceptance criteria

- Golden tests fail on unintended generator changes.
- Intentional generator changes require explicit golden updates.

## J3. Add integration tests for LSP behavior

### Goal

Prove editor-facing features without manual testing.

### Implementation

Write an LSP harness that starts `api-ls` against a fixture workspace and asserts:

- initialize succeeds,
- generated packages resolve,
- definition TS -> Rust,
- references Rust -> TS,
- hover includes route and Effect signature,
- rename produces expected Rust and TS edits,
- diagnostics include unused endpoint after TS usage is removed.

### Acceptance criteria

- LSP tests run in CI.
- Tests cover endpoint, field, error variant, and error tag behavior.

---

# Minimum next milestone

The fastest path from prototype to a credible first usable release is:

1. A1 transitive registration.
2. A2 endpoint descriptors.
3. A3 real `api collect`.
4. B2 symbol graph generation.
5. B3 precise Rust ranges for endpoints/types/fields/error variants.
6. B4 generated TS ranges.
7. D1 safe primitive/integer mapping.
8. D4 generated-package typecheck fixture.
9. E1 semantic TS references for endpoint accessors.
10. E2 Effect diagnostics for strong/weak/invalid usage.
11. F1 mandatory backend startup.
12. F2 async gateway proxy core.
13. F5 semantic cross-language definition/references.
14. F6 wire-contract rename for fields and endpoint accessors.
15. G1 `cargo api check`.
16. I1 executable npm language-server wrapper.
17. J1 one end-to-end Axum + Effect example.

When those are complete, the project will satisfy the central promise:

```text
Write the Rust API once.
Use it from Effect TypeScript with generated schemas, typed errors, and Layers.
Navigate, find usages, rename, and diagnose unused endpoints across Rust and TS.
Never edit generated files.
```

## Non-goals for the next milestone

Do not spend milestone time on these until the core loop is real:

- WebSocket duplex support beyond IR placeholders.
- Multiple framework adapters beyond Axum.
- Publishing public npm/crates.io packages.
- Full OpenAPI generation.
- Rich UI/editor extensions beyond standard LSP.
- Formatting generated TypeScript with external formatters.
- Supporting arbitrary Serde custom serializers without explicit API overrides.

## Prohibited shortcuts

These shortcuts would make demos look better while moving away from the original vision:

- Do not count text matches as endpoint usage once semantic indexing exists.
- Do not navigate users into generated files by default.
- Do not make Promise clients the primary API.
- Do not require users to manually list DTOs/errors reachable from endpoints.
- Do not allow unsupported public API shapes to silently become `unknown`.
- Do not keep running if gateway backends fail to start.
- Do not implement TS field rename as a Serde compatibility alias; it is a wire-contract rename.
- Do not make `api collect` produce empty contracts except under an explicit test/debug flag.

## Final release checklist

Before calling the implementation complete, verify this checklist:

- [ ] `cargo api collect` produces a non-empty contract from a real API root.
- [ ] `cargo api gen` writes a hidden package and symbol graph.
- [ ] Generated packages typecheck under the chosen Effect version.
- [ ] Generated runtime decodes and encodes through Effect Schema.
- [ ] `i64` and other unsafe integers are not generated as plain JS numbers by default.
- [ ] Serde-tagged errors decode to catchable tagged Effect errors.
- [ ] SSE streams fail with typed domain errors when the server sends an API error frame.
- [ ] `api-ls` fails initialization if rust-analyzer or the TS/Effect backend is unavailable.
- [ ] TS definition on endpoint/type/field/error tag jumps to Rust.
- [ ] Rust references on endpoint/type/field/error variant include TS usages.
- [ ] Rename from TS field changes the Rust wire contract and TS usages.
- [ ] Usage index is semantic and Effect-aware.
- [ ] Unused endpoint diagnostics appear in editor and CI/build mode.
- [ ] A fresh end-to-end example passes without editing generated files.
