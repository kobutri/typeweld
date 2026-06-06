# LSP Setup Guide

`typeweld-ls` is the language-server gateway. It starts and proxies Rust Analyzer and
the TypeScript language server, then adds cross-language API behavior.

Register `typeweld-ls` as the only language server for Rust and TypeScript files in
workspaces that use this tool. Do not also run a separate `rust-analyzer`,
`typescript-language-server`, `tsserver`, or Effect language-service LSP for the
same workspace. Duplicate servers can race on definitions, diagnostics, rename
edits, and generated-file visibility; `typeweld-ls` owns those backend processes
internally.

## Workspace config

Create `.typeweld.json` at the workspace root when defaults are not enough:

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
  "typeweldWatch": {
    "enabled": true,
    "command": "typeweld",
    "args": ["watch", "--manifest-path", "Cargo.toml", "--target-dir", "target"]
  },
  "effectLanguageServicePlugin": "@effect/language-service",
  "generatedCacheDir": "target/api-contract/effect-v4/packages",
  "symbolGraph": "target/api-contract/rust-ts-symbols.json",
  "usageIndex": "target/api-contract/graph/effect-usage-index.json",
  "unusedEndpointLints": "warn",
  "logFile": "target/typeweld-ls.log"
}
```

Relative paths are resolved from the discovered workspace root. `typeweld-ls` starts
the configured Rust backend as `rustAnalyzer.command` plus
`rustAnalyzer.args`, and starts the TypeScript/Effect backend as
`typescript.command` plus `typescript.args`. The default TypeScript command is
`typescript-language-server --stdio`.

`typeweldWatch` controls the generated-package refresh process owned by `typeweld-ls`.
When enabled, `typeweld-ls` starts the configured long-running watch command during
initialization, waits for its initial generation to create the cache, and keeps
the same process alive while the editor session is active. In packaged VS Code
installs, `typeweld-ls` can also discover a `typeweld` binary bundled next to itself when
`typeweldWatch.command` is empty.

## Editor command

Point your editor at the npm wrapper when using the workspace package:

```sh
npm exec --workspace @typeweld/language-server -- typeweld-ls
```

The wrapper launches the Rust gateway binary with stdio inherited unchanged. It
checks, in order:

- `TYPEWELD_LS_BINARY` or `TYPEWELD_LS`.
- A packaged binary under the wrapper package's `bin/` directory.
- Local Cargo build outputs under `target/debug` and `target/release`.
- `typeweld-ls` on `PATH`, skipping the npm wrapper itself to avoid recursion.

For local development, build the gateway first:

```sh
cargo build -p typeweld-ls --bin typeweld-ls
```

In a published install, use the installed `typeweld-ls` command directly; the npm
wrapper will use the packaged gateway binary when one is present.

## VS Code extension

The repository includes a VS Code client under
`npm/vscode-extension`. For local development, install npm dependencies and
compile the extension:

```sh
cd npm
npm install
npm run compile --workspace typeweld-vscode
```

The extension uses the bundled npm launcher by default and starts one `typeweld-ls`
client for each workspace folder with `.typeweld.json`, `typeweld.json`, or
`target/api-contract/effect-v4/packages` in that folder or one of its parents.
Add `Cargo.toml` to `typeweld.languageServer.requiredWorkspaceMarkers` if you
want defaults-only startup before generation. Override the launcher with VS Code
settings when needed:

```json
{
  "typeweld.languageServer.command": "",
  "typeweld.languageServer.args": [],
  "typeweld.languageServer.env": {
    "TYPEWELD_LS_BINARY": "/absolute/path/to/typeweld-ls"
  }
}
```

Use the `Typeweld: Restart typeweld-ls` command after changing backend
configuration or rebuilding the gateway binary.

For release packaging, see `docs/vscode-extension-release.md`.

## Generic LSP registration

For any editor or LSP client that accepts a custom server, register one server
with this shape:

```json
{
  "name": "typeweld-ls",
  "command": "typeweld-ls",
  "args": [],
  "rootPatterns": [".typeweld.json", "Cargo.toml", "package.json"],
  "languages": [
    "rust",
    "typescript",
    "typescriptreact",
    "javascript",
    "javascriptreact"
  ]
}
```

Use the npm wrapper command from the previous section in place of `typeweld-ls` when
running from a workspace checkout. Then disable or scope out the editor's normal
Rust and TypeScript language-server registrations for this workspace. The exact
setting names differ by editor, but the desired final state is:

- Rust buffers in this workspace attach to `typeweld-ls` only.
- TypeScript and JavaScript buffers in this workspace attach to `typeweld-ls` only.
- The Rust Analyzer and TypeScript/Effect backend processes are children of
  `typeweld-ls`, not separate editor-managed servers.

## Generated package refresh

The gateway serves generated package files from the configured cache directory
and keeps them refreshed through `typeweldWatch`. TypeScript should also know the
generated `paths` entries, usually by extending or copying:

```sh
target/api-contract/effect-v4/tsconfig.paths.json
```

Make sure the generated cache directory in `.typeweld.json` matches the
`typeweldWatch` target directory. A TypeScript app should extend or copy the
generated `tsconfig.paths.json`, for example:

```json
{
  "extends": "./target/api-contract/effect-v4/packages/_workspace_server-api/tsconfig.paths.json"
}
```

`cargo run -p typeweld-cli --bin typeweld -- check` still provides the stricter CI
path: it regenerates the package, typechecks it, and updates usage data for
editor diagnostics.

## Diagnostics

Backend startup is strict. During `initialize`, `typeweld-ls` starts and initializes
both configured backends. If either command is missing, exits early, or cannot
initialize, `typeweld-ls` fails initialization with a diagnostic that names the
backend, shows the configured command, and points at `.typeweld.json`.

Common startup diagnostics:

- Missing `typeweld-ls` gateway binary: the npm wrapper prints install help before
  the LSP process starts. Build with `cargo build -p typeweld-ls --bin typeweld-ls` or
  set `TYPEWELD_LS_BINARY`.
- Missing Rust backend: install `rust-analyzer`, put it on `PATH`, or set
  `rustAnalyzer.command`.
- Missing TypeScript/Effect backend: install `typescript-language-server` for
  the workspace or set `typescript.command`.
- Missing generated cache: verify `typeweldWatch.command`, `typeweldWatch.args`, and
  `generatedCacheDir`. You can also run `typeweld watch` or `typeweld check` manually to
  inspect the underlying collection error.

Set `logFile` in `.typeweld.json` to capture backend stderr under `target/` while
debugging startup failures.

Unused endpoint diagnostics require both:

- `target/api-contract/rust-ts-symbols.json`
- `target/api-contract/graph/effect-usage-index.json`

Generate the usage index with `typeweld check-usages`. Set `unusedEndpointLints` to
`off`, `warn`, or `deny` depending on how strict the editor should be.
