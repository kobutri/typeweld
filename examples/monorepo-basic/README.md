# Monorepo Basic Example

This example sketches the expected workspace flow for a Rust server package and
a TypeScript client package.

```sh
cargo run -p api-collector --bin api -- collect \
  --package-name @workspace/server-api \
  --out target/api-contract/server-api.json

cargo run -p api-collector --bin api -- gen \
  --contract target/api-contract/server-api.json \
  --target-dir target

cargo run -p api-collector --bin api -- check-usages \
  --contract target/api-contract/server-api.json \
  --out target/api-contract/graph/effect-usage-index.json \
  --ts-dir app/src
```

The generated package should stay in `target/api-contract`. Commit Rust source,
TypeScript source, and config. Do not commit generated package files.
