import assert from "node:assert/strict"
import { pathToFileURL } from "node:url"
import { Effect, Schema, Stream } from "effect"

const runtimePath = process.argv[2]
if (runtimePath === undefined) {
  throw new Error("Usage: tsx test/sse-runtime.mts <compiled-runtime-index.js>")
}

const {
  DecodeError,
  RemoteProtocolError,
  UnexpectedStatusError,
  makeSseClient,
} = await import(pathToFileURL(runtimePath).href)

class NotFound extends Schema.TaggedErrorClass()("NotFound", {
  id: Schema.Number,
}) {}

const Item = Schema.Struct({
  id: Schema.Number,
  name: Schema.String,
})

const ErrorSchemaByStatus = {
  404: NotFound,
}

const endpoint = {
  method: "GET",
  path: "/events",
  decodeSuccess: (input) => makeSseClient.decode(input, Item),
  decodeError: (status, input) => {
    const schema = ErrorSchemaByStatus[status]
    if (schema !== undefined) {
      return makeSseClient.decode(input, schema)
    }
    return undefined
  },
}

const encoder = new TextEncoder()

const responseFromSse = (body) =>
  new Response(body, {
    status: 200,
    headers: {
      "content-type": "text/event-stream",
    },
  })

const collectFromResponse = (response, transform = (stream) => stream) =>
  Effect.runPromise(
    Stream.runCollect(
      transform(
        makeSseClient({
          baseUrl: "http://api.test",
          fetch: async () => response,
        }, endpoint)({}),
      ),
    ),
  )

const failureFromResponse = async (response, transform = (stream) => stream) => {
  const exit = await Effect.runPromiseExit(
    Stream.runCollect(
      transform(
        makeSseClient({
          baseUrl: "http://api.test",
          fetch: async () => response,
        }, endpoint)({}),
      ),
    ),
  )
  const error = exit["~effect/Effect/args"]?.reasons?.[0]?.error
  assert.notEqual(error, undefined, "expected stream to fail")
  return error
}

const items = await collectFromResponse(
  responseFromSse(
    'data: {"id":1,"name":"Ada"}\n\n' +
      'event: message\ndata: {"id":2,"name":"Grace"}\n\n',
  ),
)
assert.deepEqual(items, [
  { id: 1, name: "Ada" },
  { id: 2, name: "Grace" },
])

assert.deepEqual(await collectFromResponse(responseFromSse("")), [])

const domainError = await failureFromResponse(
  responseFromSse('event: api-error\ndata: {"status":404,"body":{"_tag":"NotFound","id":9}}\n\n'),
)
assert.ok(domainError instanceof NotFound)
assert.equal(domainError.id, 9)

const malformedJson = await failureFromResponse(responseFromSse("data: not-json\n\n"))
assert.ok(malformedJson instanceof RemoteProtocolError)

const decodeError = await failureFromResponse(responseFromSse('data: {"id":"bad","name":"Ada"}\n\n'))
assert.ok(decodeError instanceof DecodeError)

const unknownStatus = await failureFromResponse(
  responseFromSse('event: api-error\ndata: {"status":418,"body":{"_tag":"NotFound","id":1}}\n\n'),
)
assert.ok(unknownStatus instanceof UnexpectedStatusError)
assert.equal(unknownStatus.status, 418)

let cancelled = false
const cancellableBody = new ReadableStream({
  start(controller) {
    controller.enqueue(encoder.encode('data: {"id":1,"name":"Ada"}\n\n'))
  },
  cancel() {
    cancelled = true
  },
})
const first = await collectFromResponse(
  new Response(cancellableBody, {
    status: 200,
    headers: {
      "content-type": "text/event-stream",
    },
  }),
  (stream) => stream.pipe(Stream.take(1)),
)
assert.deepEqual(first, [{ id: 1, name: "Ada" }])
await new Promise((resolve) => setTimeout(resolve, 0))
assert.equal(cancelled, true)

console.log("SSE runtime protocol tests passed")
