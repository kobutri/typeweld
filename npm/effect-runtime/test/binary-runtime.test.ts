import { describe, expect, it } from "@effect/vitest"
import { Effect, Schema } from "effect"

import {
  EncodeError,
  makeBinaryDownloadClient,
  makeBinaryUploadClient,
} from "../src/index"

class NotFound extends Schema.TaggedErrorClass<NotFound>()("NotFound", {
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

const failureFromEffect = (effect) => effect.pipe(Effect.flip)

describe("binary runtime protocol", () => {
  it.effect("downloads binary responses", () =>
    Effect.gen(function* () {
      let downloadRequest
      const downloaded = yield* makeBinaryDownloadClient({
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
      }, downloadEndpoint)({ id: 42 })

      expect(Array.from(downloaded)).toEqual([1, 2, 3])
      expect(downloadRequest).toEqual({
        url: "https://api.example.test/files/42",
        method: "GET",
      })
    }))

  it.effect("decodes binary download domain errors", () =>
    Effect.gen(function* () {
      const domainError = yield* failureFromEffect(
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

      expect(domainError).toBeInstanceOf(NotFound)
      expect(domainError.id).toBe(9)
    }))

  it.effect("uploads binary request bodies", () =>
    Effect.gen(function* () {
      let uploadRequest
      const uploaded = yield* makeBinaryUploadClient({
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
      }, uploadEndpoint)({ id: 7, body: new Uint8Array([4, 5, 6]) })

      expect(uploaded).toEqual({ id: 7, name: "Ada" })
      expect(uploadRequest).toEqual({
        url: "https://api.example.test/files/7",
        method: "POST",
        contentType: "application/octet-stream",
        bytes: [4, 5, 6],
      })
    }))

  it.effect("fails before fetch when binary upload bodies are invalid", () =>
    Effect.gen(function* () {
      const encodeError = yield* failureFromEffect(
        makeBinaryUploadClient({
          baseUrl: "https://api.example.test",
          fetch: async () => {
            throw new Error("fetch should not be called for invalid binary bodies")
          },
        }, uploadEndpoint)({ id: 1, body: { unsupported: true } }),
      )

      expect(encodeError).toBeInstanceOf(EncodeError)
    }))
})
