# Effect Client Guide

Generated clients expose namespaces that mirror endpoint `ts_path` metadata.
For a Rust endpoint mapped to `["users", "getUser"]`, TypeScript imports look
like this:

```ts
import { Effect } from "effect"
import { ServerApi, users } from "@workspace/server-api"

const loadUser = (id: number) =>
  users.getUser({ id }).pipe(
    Effect.catchTag("UserNotFound", () => Effect.succeed(null)),
  )
```

## Runtime layer

Provide `ServerApi.layer` at the boundary of your application:

```ts
const main = loadUser(1).pipe(
  Effect.provide(ServerApi.layer({
    baseUrl: "http://localhost:3000",
    timeoutMs: 10_000,
  })),
)
```

## Error channel semantics

Rust endpoints usually return `Result<Json<T>, E>`. The generated client maps
that to:

```ts
Effect.Effect<T, E | ApiClientError, ServerApi>
```

Domain errors such as `UserNotFound` stay typed in the Effect error channel.
Transport, encode, decode, timeout, and unexpected-status failures are surfaced
as `ApiClientError` variants.

## SSE Wire Protocol

Successful stream frames use normal SSE `data:` lines containing JSON that
matches the stream item schema. The reserved `api-error` event carries domain
errors:

```text
event: api-error
data: {"status":404,"body":{"_tag":"UserNotFound","id":1}}
```

Malformed JSON is a protocol error. A successful frame with the wrong shape is a
decode error. An `api-error` status not declared by the endpoint is an
unexpected-status error.

## Strong usage

Unused endpoint checks only count strong Effect usage. These count:

```ts
yield* users.getUser({ id })
return users.getUser({ id })
users.getUser({ id }).pipe(Effect.retry({ times: 2 }))
```

These do not count:

```ts
users.getUser
users.getUser({ id })
void users.getUser({ id })
```

Floating Effects are considered weak because they are descriptions of work, not
executed work.
