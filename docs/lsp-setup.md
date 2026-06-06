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
  "apiWatch": {
    "enabled": true,
    "command": "api",
    "args": ["watch", "--manifest-path", "Cargo.toml", "--target-dir", "target"]
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

`apiWatch` controls the generated-package refresh process owned by `api-ls`.
When enabled, `api-ls` starts the configured long-running watch command during
initialization, waits for its initial generation to create the cache, and keeps
the same process alive while the editor session is active. In packaged VS Code
installs, `api-ls` can also discover an `api` binary bundled next to itself when
`apiWatch.command` is empty.

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

## VS Code extension

The repository includes a VS Code client under
`npm/vscode-extension`. For local development, install npm dependencies and
compile the extension:

```sh
cd npm
npm install
npm run compile --workspace rust-ts-integration
```

The extension uses the bundled npm launcher by default and starts one `api-ls`
client for each workspace folder with `.api-ls.json`, `api-ls.json`, or
`target/api-contract/effect-v4/packages` in that folder or one of its parents.
Add `Cargo.toml` to `rustTsIntegration.apiLs.requiredWorkspaceMarkers` if you
want defaults-only startup before generation. Override the launcher with VS Code
settings when needed:

```json
{
  "rustTsIntegration.apiLs.command": "",
  "rustTsIntegration.apiLs.args": [],
  "rustTsIntegration.apiLs.env": {
    "API_LS_BINARY": "/absolute/path/to/api-ls"
  }
}
```

Use the `Rust TS Integration: Restart api-ls` command after changing backend
configuration or rebuilding the gateway binary.

For release packaging, see `docs/vscode-extension-release.md`.

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

## Generated package refresh

The gateway serves generated package files from the configured cache directory
and keeps them refreshed through `apiWatch`. TypeScript should also know the
generated `paths` entries, usually by extending or copying:

```sh
target/api-contract/effect-v4/tsconfig.paths.json
```

Make sure the generated cache directory in `.api-ls.json` matches the
`apiWatch` target directory. A TypeScript app should extend or copy the
generated `tsconfig.paths.json`, for example:

```json
{
  "extends": "./target/api-contract/effect-v4/packages/_workspace_server-api/tsconfig.paths.json"
}
```

`cargo run -p api-collector --bin api -- check` still provides the stricter CI
path: it regenerates the package, typechecks it, and updates usage data for
editor diagnostics.

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
- Missing generated cache: verify `apiWatch.command`, `apiWatch.args`, and
  `generatedCacheDir`. You can also run `api watch` or `api check` manually to
  inspect the underlying collection error.

Set `logFile` in `.api-ls.json` to capture backend stderr under `target/` while
debugging startup failures.

Unused endpoint diagnostics require both:

- `target/api-contract/rust-ts-symbols.json`
- `target/api-contract/graph/effect-usage-index.json`

Generate the usage index with `api check-usages`. Set `unusedEndpointLints` to
`off`, `warn`, or `deny` depending on how strict the editor should be.
