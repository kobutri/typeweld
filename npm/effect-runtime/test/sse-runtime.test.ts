import { describe, expect, it } from "@effect/vitest"
import { Effect, Schema, Stream } from "effect"

import {
  type ApiClientError,
  DecodeError,
  RemoteProtocolError,
  type SseEndpoint,
  UnexpectedStatusError,
  makeSseClient,
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

type Item = typeof Item.Type
type DomainError = NotFound
type SseError = DomainError | ApiClientError
type ItemStream = Stream.Stream<Item, SseError>

const endpoint = {
  method: "GET",
  path: "/events",
  decodeSuccess: (input: unknown) => makeSseClient.decode(input, Item),
  decodeError: (status, input) => {
    const schema = ErrorSchemaByStatus[status as keyof typeof ErrorSchemaByStatus]
    if (schema !== undefined) {
      return makeSseClient.decode(input, schema)
    }
    return undefined
  },
} satisfies SseEndpoint<Record<string, never>, Item, DomainError>

const encoder = new TextEncoder()

const responseFromSse = (body: BodyInit) =>
  new Response(body, {
    status: 200,
    headers: {
      "content-type": "text/event-stream",
    },
  })

const streamFromResponse = (response: Response): ItemStream =>
  makeSseClient({
    baseUrl: "http://api.test",
    fetch: async () => response,
  }, endpoint)({})

const collectFromResponse = (
  response: Response,
  transform: (stream: ItemStream) => ItemStream = (stream) => stream,
) =>
  Stream.runCollect(transform(streamFromResponse(response)))

const failureFromResponse = (
  response: Response,
  transform: (stream: ItemStream) => ItemStream = (stream) => stream,
) =>
  collectFromResponse(response, transform).pipe(Effect.flip)

describe("SSE runtime protocol", () => {
  it.effect("decodes message events", () =>
    Effect.gen(function* () {
      const items = yield* collectFromResponse(
        responseFromSse(
          'data: {"id":1,"name":"Ada"}\n\n' +
            'event: message\ndata: {"id":2,"name":"Grace"}\n\n',
        ),
      )

      expect(items).toEqual([
        { id: 1, name: "Ada" },
        { id: 2, name: "Grace" },
      ])
    }))

  it.effect("handles empty streams", () =>
    Effect.gen(function* () {
      const items = yield* collectFromResponse(responseFromSse(""))

      expect(items).toEqual([])
    }))

  it.effect("decodes api-error events with a known status", () =>
    Effect.gen(function* () {
      const domainError = yield* failureFromResponse(
        responseFromSse('event: api-error\ndata: {"status":404,"body":{"_tag":"NotFound","id":9}}\n\n'),
      )

      if (!(domainError instanceof NotFound)) {
        throw new Error(`Expected NotFound, received ${String(domainError)}`)
      }
      expect(domainError.id).toBe(9)
    }))

  it.effect("fails malformed JSON as a remote protocol error", () =>
    Effect.gen(function* () {
      const malformedJson = yield* failureFromResponse(responseFromSse("data: not-json\n\n"))

      expect(malformedJson).toBeInstanceOf(RemoteProtocolError)
    }))

  it.effect("fails invalid payloads as decode errors", () =>
    Effect.gen(function* () {
      const decodeError = yield* failureFromResponse(responseFromSse('data: {"id":"bad","name":"Ada"}\n\n'))

      expect(decodeError).toBeInstanceOf(DecodeError)
    }))

  it.effect("fails unknown api-error statuses", () =>
    Effect.gen(function* () {
      const unknownStatus = yield* failureFromResponse(
        responseFromSse('event: api-error\ndata: {"status":418,"body":{"_tag":"NotFound","id":1}}\n\n'),
      )

      if (!(unknownStatus instanceof UnexpectedStatusError)) {
        throw new Error(`Expected UnexpectedStatusError, received ${String(unknownStatus)}`)
      }
      expect(unknownStatus.status).toBe(418)
    }))

  it.effect("cancels the response body when the consumer stops early", () =>
    Effect.gen(function* () {
      let cancelled = false
      const cancellableBody = new ReadableStream({
        start(controller) {
          controller.enqueue(encoder.encode('data: {"id":1,"name":"Ada"}\n\n'))
        },
        cancel() {
          cancelled = true
        },
      })

      const first = yield* collectFromResponse(
        new Response(cancellableBody, {
          status: 200,
          headers: {
            "content-type": "text/event-stream",
          },
        }),
        (stream) => stream.pipe(Stream.take(1)),
      )

      expect(first).toEqual([{ id: 1, name: "Ada" }])
      yield* Effect.promise(() => new Promise((resolve) => setTimeout(resolve, 0)))
      expect(cancelled).toBe(true)
    }))
})
