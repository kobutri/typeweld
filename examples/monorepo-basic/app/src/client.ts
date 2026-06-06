// This file is the TypeScript half of the example monorepo.
//
// It intentionally imports from a package that is not checked into this
// directory. `@workspace/server-api` is generated from the Rust contract and
// resolved through `app/tsconfig.json`.

import { Effect } from "@rust-ts-integration/effect-runtime/compat"
import { ServerApi, users } from "@workspace/server-api"

// Application code writes ordinary functions around generated endpoint
// accessors. The generated `users.getUser` signature is derived from the Rust
// `get_user(Path<i64>) -> Result<Json<User>, UserError>` signature.
export const loadUserDisplayName = (id: bigint) =>
  Effect.gen(function* () {
    // `yield*` is important: this is a strong usage. The unused endpoint
    // scanner can see that the endpoint Effect is actually composed into a
    // program instead of being called and discarded.
    const user = yield* users.getUser({ id })

    // Rust field `display_name` is available as `displayName` because the Rust
    // DTO used `#[serde(rename_all = "camelCase")]`.
    return user.displayName
  })

// Domain errors are handled with normal Effect error-channel tools. They are
// not thrown promises and they are not wrapped in a success-channel Result.
export const loadUserOrGuest = (id: bigint) =>
  loadUserDisplayName(id).pipe(
    Effect.catchTag("UserNotFound", () => Effect.succeed("Guest")),
  )

// Mutating endpoints work the same way. The Rust `Body<CreateUserRequest>`
// extractor becomes a generated `body` property.
export const createUser = (displayName: string) =>
  users.createUser({ body: { displayName } })

// The generated service is supplied once at the edge of the program. Tests can
// use `ServerApi.mock`; production code usually uses `ServerApi.layer`.
export const program = loadUserOrGuest(1n).pipe(
  Effect.provide(
    ServerApi.layer({
      baseUrl: "http://localhost:3000",
      timeoutMs: 10_000,
    }),
  ),
)
