# Rename And Unused Endpoint Behavior

## Cross-language rename

Generated TypeScript names and Rust API symbols share stable symbol IDs. When a
rename starts on a linked Rust or TypeScript location, `api-ls` builds a single
workspace edit for every renamable location in the symbol graph.

Field rename is treated as a wire-contract rename. For example, renaming
`displayName` from TypeScript should move the Rust field toward a matching
Serde wire name instead of preserving the old wire key by default.

The current rename implementation validates simple identifier names and rejects
names that conflict with reserved API symbols supplied by the graph.

## Unused endpoints

An exported endpoint is unused when the usage index has no strong TypeScript
Effect usage for that endpoint and the Rust endpoint is not marked
`allow_unused`.

Strong examples:

```ts
yield* users.getUser({ id })
return users.getUser({ id })
users.getUser({ id }).pipe(Effect.retry({ times: 2 }))
```

Weak examples:

```ts
import { users } from "@workspace/server-api"
users.getUser
users.getUser({ id })
```

Build scripts can use `api-build` to turn this into Cargo warnings or
compile-time errors. The CLI can regenerate the index:

```sh
cargo run -p api-collector --bin api -- check-usages \
  --contract target/api-contract/server-api.json \
  --out target/api-contract/graph/effect-usage-index.json \
  --ts-dir app/src
```

When an endpoint is intentionally exported for external callers, mark it with
the API macro's `allow_unused` option.
