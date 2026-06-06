# Rust TS Integration for VS Code

This extension starts `api-ls` as the language server for Rust, TypeScript,
TSX, JavaScript, and JSX files in rust-ts-integration workspaces.

By default it uses the bundled `@rust-ts-integration/language-server` npm
launcher. For local development, build the Rust gateway first:

```sh
cargo build -p api-ls --bin api-ls
```

If the gateway binary is somewhere else, set:

```json
{
  "rustTsIntegration.apiLs.env": {
    "API_LS_BINARY": "/absolute/path/to/api-ls"
  }
}
```

The extension starts only for workspace folders with `.api-ls.json`,
`api-ls.json`, or `target/api-contract/effect-v4/packages` in that folder or
one of its parents. Set `rustTsIntegration.apiLs.requiredWorkspaceMarkers` to
`[]` to opt out of that guard, or add `Cargo.toml` if you want defaults-only
startup before generation.

`api-ls` owns the Rust Analyzer and TypeScript backend processes internally.
Disable or scope out duplicate editor-managed Rust and TypeScript language
servers for workspaces that use this extension.
