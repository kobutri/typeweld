# LSP Setup Guide

`api-ls` is the language-server gateway. It starts and proxies Rust Analyzer and
the TypeScript language server, then adds cross-language API behavior.

Register `api-ls` as the only language server for Rust and TypeScript files in
workspaces that use this tool. Do not also run a separate `rust-analyzer`,
`typescript-language-server`, `tsserver`, or Effect language-service LSP for the
same workspace. Duplicate servers can race on definitions, diagnostics, rename
edits, and generated-file visibility; `api-ls` owns those backend processes
internally.

## Workspace config

Create `.api-ls.json` at the workspace root when defaults are not enough:

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
  "effectLanguageServicePlugin": "@effect/language-service",
  "generatedCacheDir": "target/api-contract/effect-v4/packages",
  "symbolGraph": "target/api-contract/rust-ts-symbols.json",
  "usageIndex": "target/api-contract/graph/effect-usage-index.json",
  "unusedEndpointLints": "warn",
  "logFile": "target/api-ls.log"
}
```

Relative paths are resolved from the discovered workspace root. `api-ls` starts
the configured Rust backend as `rustAnalyzer.command` plus
`rustAnalyzer.args`, and starts the TypeScript/Effect backend as
`typescript.command` plus `typescript.args`. The default TypeScript command is
`typescript-language-server --stdio`.

## Editor command

Point your editor at the npm wrapper when using the workspace package:

```sh
npm exec --workspace @rust-ts-integration/language-server -- api-ls
```

The wrapper launches the Rust gateway binary with stdio inherited unchanged. It
checks, in order:

- `API_LS_BINARY` or `RUST_TS_API_LS`.
- A packaged binary under the wrapper package's `bin/` directory.
- Local Cargo build outputs under `target/debug` and `target/release`.
- `api-ls` on `PATH`, skipping the npm wrapper itself to avoid recursion.

For local development, build the gateway first:

```sh
cargo build -p api-ls --bin api-ls
```

In a published install, use the installed `api-ls` command directly; the npm
wrapper will use the packaged gateway binary when one is present.

## Generic LSP registration

For any editor or LSP client that accepts a custom server, register one server
with this shape:

```json
{
  "name": "api-ls",
  "command": "api-ls",
  "args": [],
  "rootPatterns": [".api-ls.json", "Cargo.toml", "package.json"],
  "languages": [
    "rust",
    "typescript",
    "typescriptreact",
    "javascript",
    "javascriptreact"
  ]
}
```

Use the npm wrapper command from the previous section in place of `api-ls` when
running from a workspace checkout. Then disable or scope out the editor's normal
Rust and TypeScript language-server registrations for this workspace. The exact
setting names differ by editor, but the desired final state is:

- Rust buffers in this workspace attach to `api-ls` only.
- TypeScript and JavaScript buffers in this workspace attach to `api-ls` only.
- The Rust Analyzer and TypeScript/Effect backend processes are children of
  `api-ls`, not separate editor-managed servers.

## Generated package reads

The gateway serves generated package files from the configured cache directory.
TypeScript should also know the generated `paths` entries, usually via:

```sh
cargo run -p api-collector --bin api -- gen \
  --contract target/api-contract/server-api.json \
  --target-dir target
```

Make sure the generated cache directory in `.api-ls.json` matches the `gen`
target directory. A TypeScript app should extend or copy the generated
`tsconfig.paths.json`, for example:

```json
{
  "extends": "./target/api-contract/effect-v4/packages/_workspace_server-api/tsconfig.paths.json"
}
```

`cargo run -p api-collector --bin api -- check` also regenerates the package,
typechecks it, and updates usage data for editor diagnostics.

## Diagnostics

Backend startup is strict. During `initialize`, `api-ls` starts and initializes
both configured backends. If either command is missing, exits early, or cannot
initialize, `api-ls` fails initialization with a diagnostic that names the
backend, shows the configured command, and points at `.api-ls.json`.

Common startup diagnostics:

- Missing `api-ls` gateway binary: the npm wrapper prints install help before
  the LSP process starts. Build with `cargo build -p api-ls --bin api-ls` or
  set `API_LS_BINARY`.
- Missing Rust backend: install `rust-analyzer`, put it on `PATH`, or set
  `rustAnalyzer.command`.
- Missing TypeScript/Effect backend: install `typescript-language-server` for
  the workspace or set `typescript.command`.
- Missing generated cache: run `api gen` or `api check` before starting the
  editor, and verify `generatedCacheDir`.

Set `logFile` in `.api-ls.json` to capture backend stderr under `target/` while
debugging startup failures.

Unused endpoint diagnostics require both:

- `target/api-contract/rust-ts-symbols.json`
- `target/api-contract/graph/effect-usage-index.json`

Generate the usage index with `api check-usages`. Set `unusedEndpointLints` to
`off`, `warn`, or `deny` depending on how strict the editor should be.
