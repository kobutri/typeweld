# E2E Axum + Effect Example

This example is the complete Rust-to-Effect loop:

- `server/` defines Rust DTOs, typed domain errors, Axum handlers, and the
  explicit API root.
- `app/` imports the hidden generated package and uses Effect accessors,
  generated schemas, domain-error tags, DTO fields, SSE streams, and endpoint
  usages.
- Generated files are written below `target/api-contract/effect-v4/packages`.
  They are disposable build output and are not checked in.

Run the full check:

```sh
./examples/e2e-axum-effect/verify.sh
```

The script runs:

```sh
cargo check -p e2e-axum-effect-server
cargo run -q -p api-collector --bin api -- check \
  --package e2e-axum-effect-server \
  --target-dir target \
  --ts-dir examples/e2e-axum-effect/app/src \
  --deny-unused-endpoints
npm --prefix examples/e2e-axum-effect/app run typecheck
npm --prefix examples/e2e-axum-effect/app run test:runtime
```

It then copies the app to `target/examples/e2e-axum-effect/without-audit`,
removes `audit-usage.ts`, and verifies that `api check --deny-unused-endpoints`
fails with an unused-endpoint diagnostic.

## Run The Server

Start the Axum server on a fixed port:

```sh
cargo run -p e2e-axum-effect-server -- serve 127.0.0.1:3000
```

Print the contract JSON directly:

```sh
cargo run -p e2e-axum-effect-server -- contract
```

The public collection path is still `api check`; the `contract` command is only
for inspecting the same contract by hand.

## TypeScript App

The app imports the generated API by package name:

```ts
import { ServerApi, events, users } from "@workspace/e2e-api"
```

`app/src/client.ts` strongly uses the JSON and SSE endpoints. It reads
`user.displayName`, writes `body.displayName`, and handles `UserNotFound` and
`DisplayNameTaken` with `Effect.catchTag`.

`app/src/audit-usage.ts` exists so the unused-endpoint test has a single usage
to remove. In normal checks every exported endpoint has at least one strong
Effect usage.

## LSP Setup Sample

Use `api-ls` as the only language server for this example workspace. Do not run
a separate Rust Analyzer or TypeScript language server for the same files.

```json
{
  "rustAnalyzer": {
    "command": "rust-analyzer",
    "args": []
  },
  "typescript": {
    "command": "typescript-language-server",
    "args": ["--stdio"]
  },
  "apiWatch": {
    "enabled": true,
    "command": "api",
    "args": [
      "watch",
      "--package",
      "e2e-axum-effect-server",
      "--target-dir",
      "target"
    ]
  },
  "generatedCacheDir": "target/api-contract/effect-v4/packages",
  "symbolGraph": "target/api-contract/rust-ts-symbols.json",
  "usageIndex": "target/api-contract/graph/effect-usage-index.json",
  "unusedEndpointLints": "deny",
  "logFile": "target/api-ls.log"
}
```

With `apiWatch` configured, the language server refreshes the package and
symbol graph when the editor starts and after Rust source changes. The same
generation path can be checked manually with:

```sh
cargo run -q -p api-collector --bin api -- check \
  --package e2e-axum-effect-server \
  --target-dir target \
  --ts-dir examples/e2e-axum-effect/app/src
```
