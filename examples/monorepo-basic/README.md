# Monorepo Basic Example

This directory is a small guided monorepo. It has a Rust "server" side and a
TypeScript "app" side, and the generated package sits between them.

The important idea: **Rust is the source of truth**. TypeScript imports a hidden
package generated from Rust metadata. Generated files live under
`target/api-contract` so they can be refreshed freely and kept out of git.

## Directory Tour

```text
examples/monorepo-basic/
├── Cargo.toml
├── src/main.rs
└── app/
    ├── package.json
    ├── tsconfig.json
    └── src/client.ts
```

- `src/main.rs` defines DTOs, typed errors, endpoints, and the explicit API
  module. It also prints the collected contract so the example is runnable.
- `app/src/client.ts` shows what application code looks like after generation:
  it imports `users` and `ServerApi` from `@workspace/server-api`.
- `app/tsconfig.json` documents the path mapping that points TypeScript at the
  hidden generated package.

## 1. Inspect the Rust Contract

The fastest confidence check is the verification script:

```sh
./examples/monorepo-basic/verify.sh
```

It runs the full loop: compile the Rust example, write contract JSON, generate
the hidden package, scan TypeScript usages, validate generated files, and
typecheck the TypeScript app against the generated package.

Run the example binary:

```sh
cargo run -p monorepo-basic
```

You should see a short status line on stderr and a pretty-printed `ApiContract`
on stdout. That JSON is the shape the generator consumes.

## 2. Write The Contract To Disk

For this example, write the contract file directly from the example binary:

```sh
mkdir -p target/api-contract
cargo run -p monorepo-basic > target/api-contract/server-api.json
```

## 3. Generate The Hidden TypeScript Package

Once you have a contract JSON file, generation uses the unified CLI:

```sh
cargo run -p api-collector --bin api -- gen \
  --contract target/api-contract/server-api.json \
  --target-dir target
```

That writes:

```text
target/api-contract/effect-v4/packages/_workspace_server-api/
```

Do not commit that directory. It is build output, like `target/debug`.

## 4. Point TypeScript At The Generated Package

The example app's `tsconfig.json` contains the path aliases TypeScript needs.
In a larger monorepo you can either extend the generated
`tsconfig.paths.json`, or copy the same `compilerOptions.paths` entries into
your app-level config.

## 5. Call Rust Endpoints From Effect

Open `app/src/client.ts`. The key line is:

```ts
const user = yield* users.getUser({ id: 1 })
```

That is a strong Effect usage. It counts as "used" for unused endpoint analysis
because the returned Effect is yielded inside an Effect program.

## 6. Build The Usage Index

After TypeScript code exists, scan it:

```sh
cargo run -p api-collector --bin api -- check-usages \
  --contract target/api-contract/server-api.json \
  --out target/api-contract/graph/effect-usage-index.json \
  --ts-dir examples/monorepo-basic/app/src
```

The usage index lets `api-ls` and `api-build` warn about exported Rust endpoints
that no Effect code actually uses.

## 7. Validate Generated State

Use `api check` in CI:

```sh
cargo run -p api-collector --bin api -- check \
  --contract target/api-contract/server-api.json \
  --target-dir target
```

If generated files are missing or stale, the command tells the developer to run
`api gen` again.

## 8. Typecheck The TypeScript App

After generation, the app should typecheck against the generated package:

```sh
npm --prefix examples/monorepo-basic/app run typecheck
```

The script uses the repository's existing TypeScript install under `npm/`.
That keeps this example self-contained inside the repo instead of requiring a
separate `npm install` in `examples/monorepo-basic/app`.

## What To Notice

- Rust endpoint signatures determine generated TypeScript argument and return
  types.
- `Result<Json<User>, UserError>` becomes
  `Effect.Effect<User, UserError | ApiClientError, ServerApi>`.
- Domain errors are values in the Effect error channel, not thrown promises.
- The explicit `api_module!` call controls what gets exported.
- Generated files are hidden because they are deterministic build output.
