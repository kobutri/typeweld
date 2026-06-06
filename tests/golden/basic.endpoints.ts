// Generated API package for @workspace/server-api
import { Effect, Stream, Schema } from "@typeweld/effect-runtime/compat"
import { ServerApi } from "./layer"
import { CreateUserRequest, User, UserEvent } from "./schemas"
import type { ApiClientError, GetUserError } from "./errors"

export namespace events {
  export interface WatchUsersArgs {}

  export const WatchUsersArgsSchema = Schema.Struct({})
  export type WatchUsersArgsEncoded = Schema.Codec.Encoded<typeof WatchUsersArgsSchema>

  export const watchUsersRoute = {
    method: "GET",
    path: "/users/events",
    transport: "ServerSentEvents",
  } as const

  export const watchUsers = (
    args: WatchUsersArgs
  ): Stream.Stream<UserEvent, ApiClientError | GetUserError, ServerApi> =>
    Stream.unwrap(ServerApi.use((api) => Effect.succeed(api.events.watchUsers(args))))

}

export namespace users {
  export interface CreateUserArgs {
    readonly body: CreateUserRequest;
  }

  export const CreateUserArgsSchema = Schema.Struct({
    body: CreateUserRequest,
  })
  export type CreateUserArgsEncoded = Schema.Codec.Encoded<typeof CreateUserArgsSchema>

  export const createUserRoute = {
    method: "POST",
    path: "/users",
    transport: "UnaryHttp",
  } as const

  export const createUser = (
    args: CreateUserArgs
  ): Effect.Effect<User, ApiClientError | GetUserError, ServerApi> =>
    ServerApi.use((api) => api.users.createUser(args))

  export interface GetUserArgs {
    readonly id: bigint;
  }

  export const GetUserArgsSchema = Schema.Struct({
    id: Schema.BigIntFromString,
  })
  export type GetUserArgsEncoded = Schema.Codec.Encoded<typeof GetUserArgsSchema>

  export const getUserRoute = {
    method: "GET",
    path: "/users/{id}",
    transport: "UnaryHttp",
  } as const

  export const getUser = (
    args: GetUserArgs
  ): Effect.Effect<User, ApiClientError | GetUserError, ServerApi> =>
    ServerApi.use((api) => api.users.getUser(args))

}
