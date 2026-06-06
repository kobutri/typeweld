# Typeweld for VS Code

This extension starts `typeweld-ls` as the language server for Rust, TypeScript,
TSX, JavaScript, and JSX files in typeweld workspaces.

By default it uses the bundled `@typeweld/language-server` npm
launcher. For local development, build the Rust gateway first:

```sh
cargo build -p typeweld-ls --bin typeweld-ls
```

If the gateway binary is somewhere else, set:

```json
{
  "typeweld.languageServer.env": {
    "TYPEWELD_LS_BINARY": "/absolute/path/to/typeweld-ls"
  }
}
```

The extension starts only for workspace folders with `.typeweld.json`,
`typeweld.json`, or `target/api-contract/effect-v4/packages` in that folder or
one of its parents. Set `typeweld.languageServer.requiredWorkspaceMarkers` to
`[]` to opt out of that guard, or add `Cargo.toml` if you want defaults-only
startup before generation.

`typeweld-ls` owns the Rust Analyzer and TypeScript backend processes internally.
Disable or scope out duplicate editor-managed Rust and TypeScript language
servers for workspaces that use this extension.
