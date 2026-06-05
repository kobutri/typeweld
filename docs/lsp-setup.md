# LSP Setup Guide

`api-ls` is the language-server gateway. It starts and proxies Rust Analyzer and
the TypeScript language server, then adds cross-language API behavior.

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
  "unusedEndpointLints": "warn"
}
```

## Editor command

Point your editor at:

```sh
cargo run -p api-ls --bin api-ls
```

In a published install, use the installed `api-ls` binary directly.

## Generated package reads

The gateway serves generated package files from the configured cache directory.
TypeScript should also know the generated `paths` entries, usually via:

```sh
cargo run -p api-collector --bin api -- gen \
  --contract target/api-contract/server-api.json \
  --target-dir target
```

## Diagnostics

Unused endpoint diagnostics require both:

- `target/api-contract/rust-ts-symbols.json`
- `target/api-contract/graph/effect-usage-index.json`

Generate the usage index with `api check-usages`. Set `unusedEndpointLints` to
`off`, `warn`, or `deny` depending on how strict the editor should be.
