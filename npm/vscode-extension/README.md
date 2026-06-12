# Typeweld for VS Code

This extension starts `typeweld lsp` as the language server for Rust,
TypeScript, and TSX files in workspaces that contain a `typeweld.toml`
manifest.

The server binary is resolved in this order:

1. the `typeweld.server.path` setting
2. the `TYPEWELD_LS_BINARY` environment variable
3. a `typeweld` binary on `PATH`
4. the platform binary bundled with the extension (marketplace installs ship
   one per supported platform, so no separate install is required)
5. local cargo builds at `target/{debug,release}/typeweld` (workspace folders
   and the repository root, for development)

For local development, build the Rust CLI first:

```sh
cargo build -p typeweld-cli --bin typeweld
```

Additional arguments for `typeweld lsp` can be configured with
`typeweld.server.args`, and protocol tracing with `typeweld.trace.server`.
The client restarts automatically when `typeweld.*` settings change, or on
demand via the "Typeweld: Restart Language Server" command.

The bundled TypeScript server plugin (`@typeweld/typescript-plugin`) filters
navigation results that point into generated packages under
`target/typeweld/packages/`.
