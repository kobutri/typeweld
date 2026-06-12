// The e2e application client: exercises every endpoint of the example API.
import process from "node:process"
import { Effect, Stream } from "effect"
import { layer } from "@workspace/e2e-api"
import {
  createUser,
  getUser,
  listUsers,
} from "@workspace/e2e-api/endpoints/users"
import { watchUsers } from "@workspace/e2e-api/endpoints/events"
import { downloadAvatar, uploadAvatar } from "@workspace/e2e-api/endpoints/avatars"
import type { User } from "@workspace/e2e-api"

export const apiLayer = (baseUrl: string) => layer({ baseUrl })

export const createAndFetch = (displayName: string) =>
  Effect.gen(function* () {
    const created = yield* createUser({ body: { displayName } })
    const fetched: User = yield* getUser({ id: created.id })
    return fetched
  }).pipe(Effect.provide(apiLayer(baseUrl())))

export const search = (query: string, limit: number) =>
  listUsers({ query, limit }).pipe(Effect.provide(apiLayer(baseUrl())))

export const firstEvents = (count: number) =>
  watchUsers().pipe(
    Stream.take(count),
    Stream.runCollect,
    Effect.provide(apiLayer(baseUrl())),
  )

export const avatarRoundTrip = (id: number, data: Uint8Array) =>
  Effect.gen(function* () {
    yield* uploadAvatar({ id, body: data })
    return yield* downloadAvatar({ id })
  }).pipe(Effect.provide(apiLayer(baseUrl())))

export const fetchMissing = (id: number) =>
  getUser({ id }).pipe(
    Effect.catchTag("UserNotFound", (error) =>
      Effect.succeed(`missing:${error.id}`),
    ),
    Effect.provide(apiLayer(baseUrl())),
  )

export const baseUrl = (): string =>
  process.env["E2E_BASE_URL"] ?? "http://127.0.0.1:39123"
