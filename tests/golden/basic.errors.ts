// Generated API package for @workspace/server-api
import { Schema } from "@rust-ts-integration/effect-runtime/compat"

export class NetworkError extends Schema.TaggedErrorClass<NetworkError>()(
  "NetworkError",
  {
    message: Schema.String,
    cause: Schema.optionalKey(Schema.Unknown),
  }
) {}

export class TimeoutError extends Schema.TaggedErrorClass<TimeoutError>()(
  "TimeoutError",
  {
    message: Schema.String,
    timeoutMs: Schema.optionalKey(Schema.Number),
  }
) {}

export class EncodeError extends Schema.TaggedErrorClass<EncodeError>()(
  "EncodeError",
  {
    message: Schema.String,
    cause: Schema.optionalKey(Schema.Unknown),
  }
) {}

export class DecodeError extends Schema.TaggedErrorClass<DecodeError>()(
  "DecodeError",
  {
    message: Schema.String,
    cause: Schema.optionalKey(Schema.Unknown),
  }
) {}

export class UnexpectedStatusError extends Schema.TaggedErrorClass<UnexpectedStatusError>()(
  "UnexpectedStatusError",
  {
    message: Schema.String,
    status: Schema.Number,
    body: Schema.optionalKey(Schema.Unknown),
  }
) {}

export class RemoteProtocolError extends Schema.TaggedErrorClass<RemoteProtocolError>()(
  "RemoteProtocolError",
  {
    message: Schema.String,
    body: Schema.optionalKey(Schema.Unknown),
  }
) {}

export type ApiClientError =
  | NetworkError
  | TimeoutError
  | EncodeError
  | DecodeError
  | UnexpectedStatusError
  | RemoteProtocolError

export class GetUserUserNotFound extends Schema.TaggedErrorClass<GetUserUserNotFound>()(
  "UserNotFound",
  {
    id: Schema.BigIntFromString,
  }
) {}

export class GetUserPermissionDenied extends Schema.TaggedErrorClass<GetUserPermissionDenied>()(
  "PermissionDenied",
  {}
) {}

export type GetUserError =
  | GetUserUserNotFound
  | GetUserPermissionDenied

export const GetUserErrorStatus = {
  UserNotFound: 404,
  PermissionDenied: 403,
} as const

export const GetUserErrorStatusByCode = {
  403: ["PermissionDenied"],
  404: ["UserNotFound"],
} as const

export const GetUserErrorSchemaByStatus = {
  403: GetUserPermissionDenied,
  404: GetUserUserNotFound,
} as const

export const GetUserErrorSymbols = {
  UserNotFound: {
    symbolId: "fixture:error:GetUserError:UserNotFound",
    rustName: "UserNotFound",
    source: {
      file: "crates/server/src/errors.rs",
      startLine: 7,
      startColumn: 3,
      endLine: 8,
      endColumn: 4,
    },
  },
  PermissionDenied: {
    symbolId: "fixture:error:GetUserError:PermissionDenied",
    rustName: "PermissionDenied",
    source: {
      file: "crates/server/src/errors.rs",
      startLine: 10,
      startColumn: 3,
      endLine: 10,
      endColumn: 20,
    },
  },
} as const
