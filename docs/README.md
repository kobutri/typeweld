# Typeweld Docs

Start here:

- `npx typeweld new my-api --yes` creates a minimal Rust + TypeScript starter.
- [Getting started](getting-started.md)
- [Axum example guide](axum-example.md)
- [Effect client guide](effect-client.md)
- [LSP setup guide](lsp-setup.md)
- [Rename and unused endpoint behavior](rename-and-unused-endpoints.md)
- [Current limitations](limitations.md)

The generated TypeScript package is treated as build output. It lives under
`target/api-contract` by default so editors and TypeScript can read it without
making generated files part of source control.
