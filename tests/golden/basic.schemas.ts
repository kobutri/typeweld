// Generated API package for @workspace/server-api
import { Schema } from "@rust-ts-integration/effect-runtime/compat"

export const CreateUserRequest = Schema.Struct({
  displayName: Schema.String,
})
export type CreateUserRequest = Schema.Schema.Type<typeof CreateUserRequest>
export type CreateUserRequestEncoded = Schema.Codec.Encoded<typeof CreateUserRequest>

export const User = Schema.Struct({
  id: Schema.BigIntFromString,
  displayName: Schema.String,
})
export type User = Schema.Schema.Type<typeof User>
export type UserEncoded = Schema.Codec.Encoded<typeof User>

export const UserEvent = Schema.Struct({
  id: Schema.BigIntFromString,
  kind: Schema.String,
})
export type UserEvent = Schema.Schema.Type<typeof UserEvent>
export type UserEventEncoded = Schema.Codec.Encoded<typeof UserEvent>
