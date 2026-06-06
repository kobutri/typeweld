# Follow-up implementation guide

This is the canonical implementation guide for turning the current prototype into the intended Rust ↔ Effect TypeScript API tool.

The guide is intentionally based on the code and docs in this repository. Effect v4 is still beta and evolves quickly, so the implementation must not rely on general public Effect documentation for v4-specific API details. Any Effect v4 syntax, imports, helper names, type aliases, or language-server behavior must be verified against the exact package version pinned by this repository and against generated-package typecheck fixtures in this repository.

## Product target

The final system is a compiler-backed Rust API contract tool with an Effect-native TypeScript surface.

Rust remains the source of truth for:

- exported endpoints,
- DTOs,
- domain errors,
- routes,
- request shapes,
- response shapes,
- stream shapes,
- wire names,
- source locations,
- unsupported-shape diagnostics.

TypeScript receives hidden generated packages that expose:

- Effect-native endpoint accessors,
- generated schemas,
- generated tagged domain errors,
- generated service and Layer helpers,
- generated stream accessors,
- generated runtime metadata for LSP and usage indexing.

Generated files live under `target/api-contract/...` and are disposable cache artifacts. Users should not edit them and should not check them in.

## Locked decisions

These decisions are fixed for the next implementation phase.

- Export starts from explicit Rust API roots, not from every public Rust item.
- Rust `Result<T, E>` maps to the Effect error channel.
- Promise clients are not the primary generated API.
- `api-ls` runs in gateway mode only.
- There is no companion/fallback LSP mode.
- `api-ls` is the single language-server entry point and delegates to rust-analyzer plus the repository-configured TypeScript/Effect backend.
- Field rename from either Rust or TypeScript is a wire-contract rename.
- Field rename updates the Rust API contract and TS usages; it is not merely a Serde compatibility alias.
- Unsupported public API shapes fail Rust compilation.
- Exported endpoints with no live Effect usage are reported in the editor and can be warned/denied in build/CI.

## Effect v4 beta policy

Effect v4 is the target, but v4-specific details must be treated as repository-controlled integration details.

The stable product model is:

```text
Rust Result<T, E>
  -> generated Effect endpoint with success T and typed error E | ApiClientError

Rust stream of Result<T, E>
  -> generated Effect stream with item T and typed error E | ApiClientError

Rust DTO / error shape
  -> generated schema-backed encoded/decoded boundary
```

The exact TypeScript code used to implement that model must be derived from this repository, not from external docs.

Repository sources of truth:

- `npm/effect-runtime/package.json` defines the runtime package dependency on Effect.
- `npm/effect-runtime/src/index.ts` defines the current runtime helpers and client error model.
- `crates/api-gen-effect-v4/src/lib.rs` defines the generated package shape.
- `crates/api-test-fixtures/src/lib.rs` defines generated-contract fixtures.
- `docs/getting-started.md` and `docs/lsp-setup.md` define the current documented workflow.
- Future generated-package fixtures must prove the actual generated TypeScript compiles against the pinned Effect beta.

Required changes for beta safety:

1. Replace broad Effect beta ranges with exact pinned versions.
2. Add a root JS package manager lockfile and use it in CI.
3. Put all version-sensitive Effect syntax behind one compatibility layer in the generator/runtime.
4. Add generated-package typecheck fixtures before claiming any generated Effect API is complete.
5. Treat Effect HttpApi/RPC emitters as optional until repository fixtures prove them against the pinned beta.
6. Do not copy v4-specific code examples from outside the repository into the generator unless the generated fixtures validate them.

## Definition of done

The implementation is complete when a fresh monorepo can author only Rust API code like this:

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

and TypeScript can use the generated API without editing generated files:

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
- `cargo api gen` materializes a hidden generated package under `target/api-contract/effect-v4/packages/...`.
- `cargo api check` runs collection, generation, generated-package typecheck, usage indexing, and lint validation.
- `api-ls` is the only configured language server for the workspace.
- TS go-to-definition on `users.getUser`, `User`, `displayName`, and `catchTag("UserNotFound")` jumps to Rust source.
- Rust find-references on an endpoint includes TS Effect call sites.
- Rust find-references on an error variant includes TS `catchTag` / `catchTags` handling sites.
- Rust find-references on a DTO field includes TS property reads and writes.
- Rename from either Rust or TS updates the wire contract and all relevant Rust/TS usages.
- Unused endpoint diagnostics count only strong/live Effect usages.
- Unsupported endpoint shapes fail rustc with precise, actionable diagnostics.

## Verified current state

| Area | Current state | Main implication |
| --- | --- | --- |
| Workspace | The workspace already has the planned crates: `api-core`, `api-ir`, `api-macros`, `api-collector`, `api-gen-effect-v4`, `api-ls`, `api-build`, `api-axum`, fixtures, and examples. | The crate split is good enough; focus on semantic completion. |
| IR | `ApiContract` currently contains `package_name`, `endpoints`, `types`, and `errors`; `Field` stores Rust/wire/TS names and a source range. | IR v1 is not enough for exact Effect channels, generated TS ranges, source maps, or wire rename. |
| Core | `ApiType`, `ApiError`, endpoint wrappers, `ApiModule`, primitive mappings, and external type mappings exist. | Core can be extended, but transitive registration is missing. |
| Macros | `ApiType`, `ApiError`, and `#[api]` exist; Serde parsing covers only a narrow subset. | Wire compatibility is not complete. |
| CLI | `api gen`, `api check`, and `api check-usages` have useful behavior; `api collect` emits an empty contract; `api watch` regenerates once. | The full Rust-source-to-TS loop is not operational. |
| Collector | `collect_contract` requires manual `types` and `errors`; usage scanning is line/string based. | Low-code export and reliable unused-endpoint detection are not implemented. |
| Effect generator | The generator emits `schemas.ts`, `errors.ts`, `endpoints.ts`, `layer.ts`, `index.ts`, `package.json`, and `tsconfig.paths.json`. | The generated package shape exists, but must be validated against a pinned beta. |
| Effect dependency | The runtime and generated package metadata currently use an Effect beta dependency range. | Broad beta ranges are unsafe; pin exact versions. |
| Type mapping | `api-core` declares unsafe integer policy, but the generator maps `i64` to `number` / `Schema.Number`. | This is a real wire-safety bug. |
| External types | `Uuid`, `DateTime<Utc>`, `Decimal`, and `JsonValue` are modeled, but the generator uses permissive declarations for external schemas. | Runtime validation is missing for important API types. |
| LSP | `api-ls` is gateway-shaped and consumes symbol/usage graph files. | It needs strict backend startup, async proxying, graph generation, and semantic cross-language behavior. |
| Symbol graph | `api-ls` and `api-build` consume `rust-ts-symbols.json`; no production producer is present. | Navigation, rename, hover, references, and lints depend on a missing core artifact. |
| Build bridge | `api-build` can read usage/symbol graphs and warn/deny. | Useful, but dependent on missing/weak graph inputs. |
| Axum adapter | Basic wrappers, domain error serialization, and SSE serialization exist. | Needs tighter route/contract coupling and end-to-end tests. |
| npm LSP wrapper | The wrapper is not an executable launcher yet. | Editor installation is not real yet. |

## Critical gaps

1. `api collect` is not compiler-backed and does not collect a real Rust API root.
2. DTO/error export is not transitive from endpoints.
3. The symbol graph required by LSP and build lints is not produced.
4. Rust source ranges and generated TS ranges are not precise enough for field-level navigation/rename.
5. Serde compatibility is too shallow for API wire correctness.
6. Generated Effect output is not typechecked against a pinned Effect v4 beta.
7. Unsafe integer and external type mappings need runtime-safe schemas.
8. Usage indexing is text-based, not TypeScript/Effect-semantic.
9. Gateway LSP is structurally present but not production-grade.
10. Wire-contract rename is documented but not implemented.
11. Workspace/multi-crate generated TS package behavior is not complete.

# Implementation tracks

Each task has independent acceptance criteria. Tracks can be implemented incrementally, but the minimum milestone at the end identifies the shortest path to a credible first usable release.

## Track A: compiler-backed contract collection

### A1. Add transitive registration to `ApiType` and `ApiError` [done]

Goal: exported endpoints automatically register every reachable request, response, stream item, and error payload type.

Implementation steps:

1. Add ordered registries to `api-core`: `TypeRegistry`, `ErrorRegistry`, and `ContractRegistry`.
2. Extend `ApiType` with `register_types(&mut TypeRegistry)`.
3. Extend `ApiError` with `register_error(&mut ContractRegistry)`.
4. Generate recursive registration in `#[derive(ApiType)]` for structs, newtypes, enums, and enum fields.
5. Implement recursive registration for wrappers and containers: `Option<T>`, `Vec<T>`, maps, `Json<T>`, `Created<T>`, `Path<T>`, `Query<T>`, `Body<T>`, and `Sse<T>`.
6. Prevent infinite recursion through registry insertion checks keyed by `SymbolId`.

Acceptance criteria:

- Examples no longer manually pass `types: vec![...]` or `errors: vec![...]`.
- Nested DTOs and error payload DTOs are exported automatically.
- Repeated shared types appear once.
- Recursive/cyclic type graphs terminate.

### A2. Replace endpoint metadata functions with endpoint descriptors [done]

Goal: each endpoint carries endpoint metadata and a typed registration function.

Implementation steps:

1. Add `EndpointDescriptor { endpoint: Endpoint, register: fn(&mut ContractRegistry) }`.
2. Change `#[api]` expansion to produce descriptors.
3. Let `api_module!` accept endpoint functions directly, not hidden helper names.
4. Keep compatibility helpers only as internal implementation details.

Acceptance criteria:

- `api_module!(name = "server", endpoints = [get_user])` works.
- The descriptor registers path, query, body, response, stream, and error types.
- Endpoint metadata remains deterministic.

### A3. Implement real `api collect` [done]

Goal: collect a real contract from a Cargo workspace package.

Implementation steps:

1. Read Cargo metadata.
2. Resolve package, target directory, feature flags, and API root.
3. Generate a temporary collector crate under `target/api-contract/collector/<package>/`.
4. Depend on the target package by path with matching features.
5. Compile and run the collector through Cargo.
6. Call the configured root function and serialize the collected contract.
7. Fail clearly if the root function is missing or has the wrong type.

Acceptance criteria:

- Default `api collect` never writes an empty contract.
- `--empty` exists only as an explicit debug/test option.
- Feature flags and cfgs match normal Cargo compilation.
- The collected contract includes endpoints and transitive types.

### A4. Read package metadata [done]

Goal: avoid repetitive CLI configuration.

Support:

```toml
[package.metadata.rust_ts]
ts_package = "@workspace/server-api"
api_root = "server::api"
features = []
```

Acceptance criteria:

- `cargo api collect --package server` works when metadata exists.
- CLI flags override metadata.
- Multiple API-enabled workspace packages are discoverable.

### A5. Build a workspace contract graph [done]

Goal: support multiple Rust crates and multiple generated TS packages.

Implementation steps:

1. Write `target/api-contract/workspace-contract.json`.
2. Record Cargo package names, TS package names, contract paths, and dependency edges.
3. Generate packages in dependency order.
4. Import shared generated packages instead of duplicating types.

Acceptance criteria:

- Multiple API crates generate multiple TS packages.
- Shared types are imported consistently.
- All package path mappings are emitted together.

## Track B: IR v2 and symbol graph

### B1. Introduce versioned IR v2 [done]

Goal: represent full API contract semantics.

Implementation steps:

1. Add `schema_version` and package/dependency metadata.
2. Replace `ResponseShape` with success-channel metadata, including status and transport.
3. Model Effect requirements explicitly where relevant.
4. Add compatibility conversion from IR v1 during transition.

Acceptance criteria:

- `Json<T>` -> JSON 200.
- `Created<T>` -> JSON 201.
- `NoContent` -> 204 and TS `void`/`undefined` success.
- `Sse<T>` -> stream success.
- Binary upload/download can be represented without another breaking IR change.

### B2. Produce `rust-ts-symbols.json` [done]

Goal: generate the graph consumed by `api-ls` and `api-build`.

Implementation steps:

1. Emit a symbol for every endpoint, type, field, enum variant, error variant, error tag, route path param, generated TS accessor, and generated TS schema declaration.
2. Include stable `SymbolId`, kind, Rust name range, Rust full range, generated TS range, and metadata.
3. Include generated locations and user usage locations separately.
4. Make output deterministic.

Acceptance criteria:

- `api gen` writes the symbol graph every time.
- `api-ls` no longer depends on hand-authored graph fixtures.
- Generated declarations can redirect to Rust source.

### B3. Add precise Rust source ranges

Goal: field-level navigation and rename must land on identifiers.

Implementation steps:

1. Map Rust module paths to source files during collection.
2. Parse source files and match collected symbols to items/fields/variants.
3. Compute zero-based UTF-16 LSP ranges.
4. Store name ranges and full ranges.

Acceptance criteria:

- TS definition on `displayName` lands on `display_name`.
- TS definition on an error tag lands on the Rust enum variant.
- Rename edits only the relevant identifier/range.

### B4. Track generated TypeScript ranges

Goal: generated TS declarations must map back to Rust.

Implementation steps:

1. Replace ad hoc string concatenation with a `TrackedWriter`.
2. Mark schema identifiers, type aliases, fields, endpoint accessors, route metadata, error classes, error tags, status metadata, and service methods.
3. Persist these marks into the symbol graph.

Acceptance criteria:

- TS ranges exist for every generated public symbol.
- Navigation from generated symbols resolves to Rust source.

## Track C: macros, Serde, and compile-time validation

### C1. Upgrade Serde interpretation

Goal: generated wire shapes should match Serde for API-relevant cases.

Implementation steps:

1. Use a robust Serde attribute parser instead of one-off parsing where possible.
2. Support rename, rename_all, variant rename rules, tag/content, untagged, flatten, default, skip, alias, transparent, deny_unknown_fields, and selected custom serializer cases.
3. Fail on unsupported custom serializers unless an explicit API override is provided.

Acceptance criteria:

- Serde enum tagging tests compare Rust JSON with generated schema expectations.
- Flatten/default/skip behavior is represented or rejected precisely.
- Unsupported serializers fail with actionable diagnostics.

### C2. Fix optionality and nullability

Goal: distinguish missing and null.

Implementation steps:

1. Add `OptionalNullable` to optionality.
2. Distinguish response encoding from request decoding where defaults exist.
3. Generate the pinned Effect schema equivalent of nullable, optional, and optional-nullable fields.

Acceptance criteria:

- Plain `Option<T>` and omitted `Option<T>` produce different encoded shapes.
- Defaults are accepted on decode when Rust would accept them.

### C3. Require typed public errors

Goal: keep generated error channels finite and catchable.

Implementation steps:

1. Reject `anyhow::Error`, `Box<dyn Error>`, `String`, `std::io::Error`, and opaque errors in public endpoints.
2. Require `ApiError` or an explicit mapping adapter.
3. Provide compile-fail tests and help text.

Acceptance criteria:

- Public endpoint errors must be typed domain enums or mapped explicitly.
- Shared and endpoint-specific error enums both work.

### C4. Validate endpoint signatures

Goal: catch route/request/transport mistakes at Rust compile time.

Implementation steps:

1. Verify every route param has exactly one matching `Path<T>` argument.
2. Reject extra `Path<T>` args.
3. Reject duplicate body extractors.
4. Enforce method/body policy.
5. Validate SSE return wrappers.
6. Reject unsupported extractors with help text.

Acceptance criteria:

- trybuild covers missing path param, extra path param, duplicate body, invalid SSE return, and unsupported extractor.

## Track D: Effect v4 generator and runtime correctness

### D0. Pin Effect v4 beta and isolate version-sensitive API syntax

Goal: make beta usage reproducible and repo-authoritative.

Implementation steps:

1. Replace Effect beta ranges in `npm/effect-runtime/package.json` and generated package metadata with exact versions.
2. Add root package-manager files and a lockfile.
3. Create a small compatibility layer in the generator/runtime for version-sensitive syntax: schema helpers, tagged errors, Context/Service, Layer, Stream, and Effect helpers.
4. Add a checklist for intentionally bumping the pinned Effect beta.
5. Require generated-package typecheck fixtures to pass before merging a beta bump.

Acceptance criteria:

- Runtime and generated packages use the same exact Effect version.
- Updating the Effect beta changes one compatibility layer plus fixture expectations.
- The guide and code do not rely on external v4 examples.

### D1. Fix primitive and integer wire mappings

Goal: avoid unsafe JavaScript numbers.

Implementation steps:

1. Map safe-width integers and floats to numbers.
2. Map `i64`, `u64`, `i128`, `u128`, `usize`, and `isize` to string-encoded schemas by default.
3. Add explicit opt-in for unsafe number encoding.
4. Use pinned-version schema transforms when decoded and encoded forms differ.

Acceptance criteria:

- `i64` no longer renders as `Schema.Number` by default.
- Request encoding and response decoding preserve large integers.

### D2. Replace permissive external schemas

Goal: validate important wire types.

Implementation steps:

1. Generate real schemas for UUID, date/time, decimal, JSON value, and bytes.
2. Decode to branded strings first; richer decoded types can be a config option.
3. Remove permissive always-true declarations for standard external API types.

Acceptance criteria:

- Invalid UUID/date/decimal payloads fail decoding.
- Encoded and decoded types are explicit.

### D3. Encode requests through schemas

Goal: do not send decoded values directly when wire shape differs.

Implementation steps:

1. Generate argument schemas for every endpoint.
2. Encode path, query, headers, and body values before constructing fetch requests.
3. Map encode failures into `EncodeError`.

Acceptance criteria:

- Branded/newtype/date/decimal values encode correctly.
- Path/query/body values are encoded consistently.

### D4. Add generated-package typecheck fixtures

Goal: prove generated code compiles against the pinned beta.

Implementation steps:

1. Render generated packages into temp fixture workspaces.
2. Link the local runtime package.
3. Run `tsc --noEmit` and the configured Effect-aware language diagnostics.
4. Include consumer examples with endpoint calls, domain-error handling, Layer provisioning, and streams.

Acceptance criteria:

- Unary endpoints, domain errors, SSE streams, optional fields, newtypes, external types, and multiple namespaces typecheck.
- Generated domain errors are catchable under the pinned beta.

### D5. Harden domain error decoding

Goal: decode declared errors exactly.

Implementation steps:

1. Select candidate error schemas by HTTP status.
2. Use `_tag` when multiple variants share a status.
3. Return `UnexpectedStatusError` for undeclared statuses.
4. Return `DecodeError` for known status with invalid body.

Acceptance criteria:

- Multiple variants may share a status if tags differ.
- Known status plus invalid body is a decode failure.
- Tagged domain errors narrow correctly in TS fixtures.

### D6. Harden SSE protocol

Goal: reliable stream success/error handling.

Implementation steps:

1. Specify normal event and domain-error event formats.
2. Decode every data frame through schemas.
3. Convert malformed frames to protocol/decode errors.
4. Fail the stream with typed domain errors when the server sends an API error frame.

Acceptance criteria:

- Runtime tests cover normal events, domain errors, malformed JSON, unknown status, cancellation, and stream close.

## Track E: semantic TypeScript and Effect usage indexing

### E1. Replace line scanning with semantic TypeScript references

Goal: detect real endpoint references.

Implementation steps:

1. Load the user TS project with generated path mappings.
2. Resolve generated endpoint accessor symbols.
3. Find references through TypeScript symbol resolution.
4. Map references to endpoint IDs.

Acceptance criteria:

- Aliased imports, reexports, destructuring, and wrappers work.
- Type-only references do not count as runtime usage.
- Same property name on unrelated values does not count.

### E2. Integrate the repository-configured Effect-aware diagnostics

Goal: classify live Effect usage, not text occurrences.

Implementation steps:

1. Run the TypeScript backend configured for this repository.
2. Ingest diagnostics produced by that backend.
3. Classify each endpoint reference as strong, weak, invalid, or unknown.
4. Keep diagnostic-code handling configurable because the Effect beta and language tooling may change.

Acceptance criteria:

- A bare endpoint call by itself is not strong usage.
- A yielded endpoint call is strong usage.
- Returning an endpoint Effect is strong usage.
- Piped/composed endpoint effects are strong only when semantic analysis confirms the value is an Effect.

### E3. Index error handling and field usage

Goal: support references beyond endpoints.

Implementation steps:

1. Index domain-error handling references by resolving generated error symbols and tag values.
2. Index DTO property reads/writes through TypeScript types.
3. Index endpoint argument fields and route constants where exposed.

Acceptance criteria:

- Rust references on an error variant include TS handlers.
- Rust references on a field include only typed usages of that field.

### E4. Persist a rich usage index

Goal: make LSP/build diagnostics reproducible.

Implementation steps:

1. Add schema version, contract hash, TS program hash, summaries, usages, diagnostics, and staleness metadata.
2. Stop relying only on file modification times.
3. Support incremental updates in `api-ls`.

Acceptance criteria:

- Missing, stale, and valid indexes are distinguishable.
- Build lints consume the same usage semantics as the editor.

## Track F: gateway-only LSP

### F1. Make backend startup mandatory

Goal: honor gateway-only mode.

Implementation steps:

1. Fail `initialize` if rust-analyzer cannot start or initialize.
2. Fail `initialize` if the configured TypeScript/Effect backend cannot start or initialize.
3. Include the configured command and remediation hint in the error.

Acceptance criteria:

- No soft fallback exists.
- Missing backends fail initialization clearly.

### F2. Replace synchronous proxying with an async JSON-RPC core

Goal: handle real editor concurrency.

Implementation steps:

1. Use async IO for client and backend streams.
2. Maintain request ID maps between client and backends.
3. Forward notifications and cancellations.
4. Capture backend stderr into logs.
5. Implement clean shutdown/exit.

Acceptance criteria:

- Concurrent Rust and TS requests do not block each other.
- Backend notifications are forwarded promptly.
- `$/cancelRequest` is forwarded.

### F3. Merge backend capabilities

Goal: expose normal Rust/TS features plus cross-language behavior.

Implementation steps:

1. Initialize both backends.
2. Inspect their capabilities.
3. Advertise merged capabilities plus cross-language overrides.

Acceptance criteria:

- Formatting, completion, semantic tokens, code actions, call hierarchy, and workspace symbols work where backends support them.
- Cross-language definition/references/rename/hover are handled by `api-ls`.

### F4. Serve generated files without navigating users into them

Goal: generated files are available to TS but transparent to users.

Implementation steps:

1. Ensure generated packages exist before initializing the TS backend.
2. Provide TS path mappings for generated packages.
3. Redirect generated-symbol definitions to Rust source by default.
4. Add an explicit command for opening generated files when needed.

Acceptance criteria:

- TS backend resolves generated imports.
- User go-to-definition does not land in generated files by default.

### F5. Implement semantic cross-language definition and references

Goal: make navigation work without hand-authored graph fixtures.

Implementation steps:

1. Use the generated symbol graph and backend semantic references.
2. Implement TS user usage -> generated symbol -> Rust source.
3. Implement Rust symbol -> generated symbol -> TS semantic references.
4. Cover endpoints, DTOs, fields, error variants, error tags, and endpoint argument fields.

Acceptance criteria:

- Definition and references work bidirectionally for the core symbol kinds.
- Results are deduplicated and hide generated locations by default.

### F6. Implement wire-contract rename

Goal: rename from either language changes the public API contract intentionally.

Implementation steps:

1. TS field rename updates Rust field name when possible.
2. If container rename rules express the new wire name, avoid redundant `serde(rename)`.
3. If not, add/update the correct Serde rename attribute.
4. Route param rename updates route path, handler argument, request args, and TS usages.
5. Endpoint function rename updates TS accessor and TS usages, but does not change HTTP method/path unless the route symbol is renamed explicitly.
6. Error variant rename updates tag, generated error class, and TS handlers.
7. Regenerate cache after source edits.

Acceptance criteria:

- Rename has conflict detection for sibling fields, endpoint names, error tags, and route params.
- Generated files are not edited directly.
- Prepare-rename rejects unsafe positions with a helpful reason.

## Track G: build, CI, and command workflow

### G1. Implement `cargo api check`

Goal: one command validates the whole system.

Implementation steps:

1. Collect workspace contracts.
2. Generate hidden packages and symbol graphs.
3. Typecheck generated packages.
4. Run configured TypeScript/Effect diagnostics.
5. Update semantic usage indexes.
6. Validate graph/index freshness.
7. Run usage lints according to policy.
8. Optionally run `cargo check` with generated lint glue enabled.

Acceptance criteria:

- A fresh end-to-end example passes with one command.
- Stale generated output fails with actionable instructions.
- `--deny-unused-endpoints` fails when an exported endpoint has no strong TS usage.

### G2. Strengthen build lint bridge

Goal: make Rust-side unused endpoint feedback reliable.

Implementation steps:

1. Use contract/index hashes rather than modification times only.
2. Distinguish missing graph, missing usage index, stale usage index, and zero usage.
3. Print Cargo warnings in warn mode.
4. Emit `compile_error!` only in deny mode.
5. Include endpoint route/accessor in every diagnostic.

Acceptance criteria:

- Warn mode never fails builds.
- Deny mode fails builds only for configured hard failures.
- Diagnostics point to the exact regeneration command.

### G3. Add CI workflows

Goal: prevent regressions.

Implementation steps:

1. Add Rust formatting, clippy, tests, and trybuild checks.
2. Add JS package install/typecheck checks.
3. Add generated-package typecheck fixtures.
4. Add end-to-end example checks.
5. Add LSP harness checks once `api-ls` is async/semantic.

Acceptance criteria:

- CI exercises Rust -> generated package -> TS consumer.
- CI fails if generated TS no longer typechecks.

## Track H: Axum and transport integration

### H1. Couple route registration to endpoint descriptors

Goal: avoid divergence between API metadata and actual Axum routes.

Implementation steps:

1. Add route helpers that consume endpoint descriptors.
2. Register Axum routes from descriptor method/path.
3. Reject accidental method/path divergence unless explicitly overridden.

Acceptance criteria:

- Route metadata and router behavior match in tests.
- A handler cannot silently be mounted under a different contract path.

### H2. Align framework wrappers with core wrappers

Goal: remove confusion between similarly named wrappers.

Implementation steps:

1. Choose a single wrapper policy for `Json`, `Path`, `Query`, `Body`, `Sse`, and `Created`.
2. Ensure `#[api]` recognizes the actual wrappers used in Axum handlers.
3. Update examples and docs.

Acceptance criteria:

- User examples do not juggle unrelated wrapper types.
- Axum handlers and API metadata use consistent shapes.

### H3. Expand transports after unary + SSE are solid

Implementation order:

1. Unary JSON HTTP.
2. SSE server streams.
3. Binary download.
4. Binary upload.
5. Multipart.
6. WebSocket duplex.

Acceptance criteria:

- Each transport has Rust adapter tests, generated TS typecheck tests, and runtime tests.
- Unsupported transport shapes fail at compile time.

## Track I: npm packaging and editor installation

### I1. Make the language-server wrapper executable

Goal: editors can launch `api-ls` consistently.

Implementation steps:

1. Add an npm `bin` entry for `api-ls`.
2. Locate a local or packaged `api-ls` binary.
3. Forward stdio untouched.
4. Print clear install diagnostics when missing.

Acceptance criteria:

- The wrapper starts the gateway from an editor or command line.
- Wrapper tests verify argument forwarding and failure messages.

### I2. Add root package-manager files

Goal: make TS checks reproducible.

Implementation steps:

1. Add root `package.json`.
2. Add chosen workspace config.
3. Add lockfile.
4. Add scripts for runtime typecheck, generated fixture typecheck, and LSP wrapper tests.

Acceptance criteria:

- Fresh checkout can run documented TS commands.
- CI uses the same commands as developers.

### I3. Document gateway-only editor setup

Goal: prevent duplicate language servers.

Implementation steps:

1. Document how to register `api-ls` as the only server for Rust and TS in this workspace.
2. Document how `api-ls` starts rust-analyzer and the TS/Effect backend.
3. Document generated package path setup.
4. Document backend startup diagnostics.

Acceptance criteria:

- Docs explicitly say not to run separate rust-analyzer or TypeScript LSP for workspaces using this tool.
- At least one generic LSP setup is documented, plus editor-specific examples where useful.

## Track J: end-to-end examples and tests

### J1. Add a runnable Axum + Effect example

Goal: prove the complete product loop.

Implementation steps:

Create:

```text
examples/e2e-axum-effect/
  server/
  app/
  README.md
```

Include:

- Rust DTOs and errors,
- unary endpoint,
- SSE endpoint,
- generated hidden package,
- TS Effect consumer,
- domain-error handling,
- field usage,
- endpoint usage,
- unused endpoint test,
- LSP setup sample.

Acceptance criteria:

- `cargo api check` passes.
- Generated TS typechecks.
- Axum server responses match generated schemas.
- Removing TS usage causes unused endpoint diagnostics.

### J2. Add golden outputs

Goal: make generator behavior reviewable.

Implementation steps:

Store expected outputs for representative contracts:

```text
tests/golden/basic.contract.json
tests/golden/basic.schemas.ts
tests/golden/basic.errors.ts
tests/golden/basic.endpoints.ts
tests/golden/basic.symbols.json
```

Acceptance criteria:

- Golden tests fail on unintended generator changes.
- Intentional generator changes require explicit golden updates.

### J3. Add LSP integration tests

Goal: prove editor-facing behavior without manual testing.

Implementation steps:

Write an LSP harness that starts `api-ls` against a fixture workspace and asserts:

- initialize succeeds,
- generated packages resolve,
- definition TS -> Rust,
- references Rust -> TS,
- hover includes route and generated signature,
- rename produces expected Rust and TS edits,
- diagnostics include unused endpoint after TS usage is removed.

Acceptance criteria:

- LSP tests run in CI.
- Tests cover endpoint, field, error variant, and error tag behavior.

# Minimum next milestone

The fastest path from prototype to a credible first usable release is:

1. A1 transitive registration.
2. A2 endpoint descriptors.
3. A3 real `api collect`.
4. B2 symbol graph generation.
5. B3 precise Rust ranges for endpoints/types/fields/error variants.
6. B4 generated TS ranges.
7. D0 exact Effect beta pinning and compatibility layer.
8. D1 safe primitive/integer mapping.
9. D4 generated-package typecheck fixture.
10. E1 semantic TS references for endpoint accessors.
11. E2 repository-configured Effect diagnostics for strong/weak/invalid usage.
12. F1 mandatory backend startup.
13. F2 async gateway proxy core.
14. F5 semantic cross-language definition/references.
15. F6 wire-contract rename for fields and endpoint accessors.
16. G1 `cargo api check`.
17. I1 executable npm language-server wrapper.
18. J1 one end-to-end Axum + Effect example.

When those are complete, the project will satisfy the central promise:

```text
Write the Rust API once.
Use it from generated Effect TypeScript with schemas, typed errors, and Layers.
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

- Do not count text matches as endpoint usage once semantic indexing exists.
- Do not navigate users into generated files by default.
- Do not make Promise clients the primary API.
- Do not require users to manually list DTOs/errors reachable from endpoints.
- Do not allow unsupported public API shapes to silently become `unknown`.
- Do not keep running if gateway backends fail to start.
- Do not implement TS field rename as a Serde compatibility alias.
- Do not make `api collect` produce empty contracts except under an explicit test/debug flag.
- Do not rely on external Effect docs for v4-specific API syntax; validate against this repository's pinned package and fixtures.

## Final release checklist

- [ ] `cargo api collect` produces a non-empty contract from a real API root.
- [ ] `cargo api gen` writes a hidden package and symbol graph.
- [ ] Generated packages typecheck under the repository-pinned Effect version.
- [ ] Generated runtime decodes and encodes through generated schemas.
- [ ] `i64` and other unsafe integers are not generated as plain JS numbers by default.
- [ ] Serde-tagged errors decode to catchable tagged generated errors.
- [ ] SSE streams fail with typed domain errors when the server sends an API error frame.
- [ ] `api-ls` fails initialization if rust-analyzer or the TS/Effect backend is unavailable.
- [ ] TS definition on endpoint/type/field/error tag jumps to Rust.
- [ ] Rust references on endpoint/type/field/error variant include TS usages.
- [ ] Rename from TS field changes the Rust wire contract and TS usages.
- [ ] Usage index is semantic and Effect-aware.
- [ ] Unused endpoint diagnostics appear in editor and CI/build mode.
- [ ] A fresh end-to-end example passes without editing generated files.
