import assert from "node:assert/strict"
import { pathToFileURL } from "node:url"
import { Effect, Schema } from "effect"

const runtimePath = process.argv[2]
if (runtimePath === undefined) {
  throw new Error("Usage: tsx test/binary-runtime.mts <compiled-runtime-index.js>")
}

const {
  EncodeError,
  makeBinaryDownloadClient,
  makeBinaryUploadClient,
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

const decodeError = (helper) => (status, input) => {
  const schema = ErrorSchemaByStatus[status]
  if (schema !== undefined) {
    return helper.decode(input, schema)
  }
  return undefined
}

const downloadEndpoint = {
  method: "GET",
  path: "/files/{id}",
  encode: (args) => ({ path: { id: args.id } }),
  decodeError: decodeError(makeBinaryDownloadClient),
}

const uploadEndpoint = {
  method: "POST",
  path: "/files/{id}",
  encode: (args) => ({
    path: { id: args.id },
    body: args.body,
    bodyKind: "binary",
  }),
  decodeSuccess: (input) => makeBinaryUploadClient.decode(input, Item),
  decodeError: decodeError(makeBinaryUploadClient),
}

const failureFromEffect = async (effect) => {
  const exit = await Effect.runPromiseExit(effect)
  const error = exit["~effect/Effect/args"]?.reasons?.[0]?.error
  assert.notEqual(error, undefined, "expected effect to fail")
  return error
}

let downloadRequest
const downloaded = await Effect.runPromise(
  makeBinaryDownloadClient({
    baseUrl: "https://api.example.test",
    fetch: async (url, init) => {
      downloadRequest = { url: String(url), method: init?.method }
      return new Response(new Uint8Array([1, 2, 3]), {
        status: 200,
        headers: {
          "content-type": "application/octet-stream",
        },
      })
    },
  }, downloadEndpoint)({ id: 42 }),
)

assert.deepEqual(Array.from(downloaded), [1, 2, 3])
assert.deepEqual(downloadRequest, {
  url: "https://api.example.test/files/42",
  method: "GET",
})

const domainError = await failureFromEffect(
  makeBinaryDownloadClient({
    baseUrl: "https://api.example.test",
    fetch: async () =>
      new Response(JSON.stringify({ _tag: "NotFound", id: 9 }), {
        status: 404,
        headers: {
          "content-type": "application/json",
        },
      }),
  }, downloadEndpoint)({ id: 9 }),
)
assert.ok(domainError instanceof NotFound)
assert.equal(domainError.id, 9)

let uploadRequest
const uploaded = await Effect.runPromise(
  makeBinaryUploadClient({
    baseUrl: "https://api.example.test",
    fetch: async (url, init) => {
      const request = new Request(url, init)
      uploadRequest = {
        url: String(url),
        method: init?.method,
        contentType: request.headers.get("content-type"),
        bytes: Array.from(new Uint8Array(await request.arrayBuffer())),
      }
      return new Response(JSON.stringify({ id: 7, name: "Ada" }), {
        status: 200,
        headers: {
          "content-type": "application/json",
        },
      })
    },
  }, uploadEndpoint)({ id: 7, body: new Uint8Array([4, 5, 6]) }),
)

assert.deepEqual(uploaded, { id: 7, name: "Ada" })
assert.deepEqual(uploadRequest, {
  url: "https://api.example.test/files/7",
  method: "POST",
  contentType: "application/octet-stream",
  bytes: [4, 5, 6],
})

const encodeError = await failureFromEffect(
  makeBinaryUploadClient({
    baseUrl: "https://api.example.test",
    fetch: async () => {
      throw new Error("fetch should not be called for invalid binary bodies")
    },
  }, uploadEndpoint)({ id: 1, body: { unsupported: true } }),
)
assert.ok(encodeError instanceof EncodeError)

console.log("Binary runtime protocol tests passed")
