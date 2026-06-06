import { Effect, Stream } from "@rust-ts-integration/effect-runtime/compat"
import { ServerApi, events, users } from "@workspace/e2e-api"

export const serverLayer = (baseUrl: string) =>
  ServerApi.layer({
    baseUrl,
    timeoutMs: 5_000,
  })

export const loadUserDisplayName = (id: number) =>
  Effect.gen(function* () {
    const user = yield* users.getUser({ id })
    return user.displayName
  })

export const loadUserOrGuest = (id: number) =>
  loadUserDisplayName(id).pipe(
    Effect.catchTag("UserNotFound", () => Effect.succeed("Guest")),
  )

export const createUser = (displayName: string) =>
  users.createUser({
    body: {
      displayName,
    },
  })

export const createUserOrConflictName = (displayName: string) =>
  createUser(displayName).pipe(
    Effect.map((user) => user.displayName),
    Effect.catchTag("DisplayNameTaken", (error) =>
      Effect.succeed(error.display_name),
    ),
  )

export const collectUserEventKinds = events.watchUsers({}).pipe(
  Stream.take(2),
  Stream.runCollect,
  Effect.map((items) => Array.from(items, (item) => item.kind)),
)

export const program = (baseUrl: string) =>
  Effect.gen(function* () {
    const displayName = yield* loadUserDisplayName(1)
    const guestName = yield* loadUserOrGuest(404)
    const createdName = yield* createUser("Grace Hopper").pipe(
      Effect.map((user) => user.displayName),
    )
    const conflictName = yield* createUserOrConflictName("Root")
    const eventKinds = yield* collectUserEventKinds

    return {
      displayName,
      guestName,
      createdName,
      conflictName,
      eventKinds,
    }
  }).pipe(Effect.provide(serverLayer(baseUrl)))
