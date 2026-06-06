import { Effect, Schema, Stream } from "./compat.js"

export type HttpMethod = "DELETE" | "GET" | "PATCH" | "POST" | "PUT"

export type RequestValue = string | number | bigint | boolean | null | undefined

export interface FetchClientConfig {
  readonly baseUrl: string
  readonly fetch?: typeof fetch
  readonly headers?: HeadersInit
  readonly timeoutMs?: number
}

export interface EncodedRequest {
  readonly path?: Readonly<Record<string, RequestValue>>
  readonly query?: Readonly<Record<string, RequestValue | ReadonlyArray<RequestValue>>>
  readonly body?: unknown
  readonly headers?: HeadersInit
}

export interface UnaryHttpEndpoint<Args, Success, DomainError> {
  readonly method: HttpMethod
  readonly path: string
  readonly encode?: (args: Args) => EncodedRequest
  readonly decodeSuccess: (input: unknown) => Effect.Effect<Success, ApiClientError>
  readonly decodeError?: (
    status: number,
    input: unknown,
  ) => Effect.Effect<DomainError, ApiClientError> | undefined
}

export interface SseEndpoint<Args, Success, DomainError> {
  readonly method: HttpMethod
  readonly path: string
  readonly encode?: (args: Args) => EncodedRequest
  readonly decodeSuccess: (input: unknown) => Effect.Effect<Success, ApiClientError>
  readonly decodeError?: (
    status: number,
    input: unknown,
  ) => Effect.Effect<DomainError, ApiClientError> | undefined
}

export class NetworkError extends Schema.TaggedErrorClass<NetworkError>()(
  "NetworkError",
  {
    message: Schema.String,
    cause: Schema.optionalKey(Schema.Unknown),
  },
) {}

export class TimeoutError extends Schema.TaggedErrorClass<TimeoutError>()(
  "TimeoutError",
  {
    message: Schema.String,
    timeoutMs: Schema.optionalKey(Schema.Number),
  },
) {}

export class EncodeError extends Schema.TaggedErrorClass<EncodeError>()(
  "EncodeError",
  {
    message: Schema.String,
    cause: Schema.optionalKey(Schema.Unknown),
  },
) {}

export class DecodeError extends Schema.TaggedErrorClass<DecodeError>()(
  "DecodeError",
  {
    message: Schema.String,
    cause: Schema.optionalKey(Schema.Unknown),
  },
) {}

export class UnexpectedStatusError extends Schema.TaggedErrorClass<UnexpectedStatusError>()(
  "UnexpectedStatusError",
  {
    message: Schema.String,
    status: Schema.Number,
    body: Schema.optionalKey(Schema.Unknown),
  },
) {}

export class RemoteProtocolError extends Schema.TaggedErrorClass<RemoteProtocolError>()(
  "RemoteProtocolError",
  {
    message: Schema.String,
    body: Schema.optionalKey(Schema.Unknown),
  },
) {}

export type ApiClientError =
  | NetworkError
  | TimeoutError
  | EncodeError
  | DecodeError
  | UnexpectedStatusError
  | RemoteProtocolError

export const decodeWithSchema = <S extends Schema.Top>(
  input: unknown,
  schema: S,
): Effect.Effect<S["Type"], DecodeError> =>
  Schema.decodeUnknownEffect(schema)(input).pipe(
    Effect.mapError(
      (cause) =>
        new DecodeError({
          message: "Failed to decode API response",
          cause,
        }),
    ),
  ) as Effect.Effect<S["Type"], DecodeError>

export const encodeWithSchema = <S extends Schema.Encoder<unknown>>(
  input: S["Type"],
  schema: S,
): S["Encoded"] => Schema.encodeSync(schema)(input) as S["Encoded"]

export const makeUnaryHttpClient = Object.assign(
  <Args, Success, DomainError>(
    config: FetchClientConfig,
    endpoint: UnaryHttpEndpoint<Args, Success, DomainError>,
  ): ((args: Args) => Effect.Effect<Success, DomainError | ApiClientError>) =>
    Effect.fn("makeUnaryHttpClient.endpoint")(function* (
      args: Args,
    ): Effect.fn.Return<Success, DomainError | ApiClientError> {
      const encoded = yield* Effect.try({
        try: () => endpoint.encode?.(args) ?? {},
        catch: (cause) =>
          new EncodeError({
            message: "Failed to encode API request",
            cause,
          }),
      })

      const response = yield* fetchResponse(config, endpoint, encoded)
      const body = yield* readResponseBody(response)

      if (response.ok) {
        return yield* endpoint.decodeSuccess(body)
      }

      const domainError = endpoint.decodeError?.(response.status, body)
      if (domainError !== undefined) {
        const decodedError = yield* domainError
        return yield* Effect.fail(decodedError)
      }

      return yield* new UnexpectedStatusError({
        message: `Unexpected HTTP status ${response.status}`,
        status: response.status,
        body,
      })
    }),
  {
    decode: decodeWithSchema,
    encode: encodeWithSchema,
  },
)

export const makeSseClient = Object.assign(
  <Args, Success, DomainError>(
    config: FetchClientConfig,
    endpoint: SseEndpoint<Args, Success, DomainError>,
  ): ((args: Args) => Stream.Stream<Success, DomainError | ApiClientError>) =>
    (args: Args): Stream.Stream<Success, DomainError | ApiClientError> =>
      Stream.unwrap(
        Effect.gen(function* () {
          const encoded = yield* Effect.try({
            try: () => endpoint.encode?.(args) ?? {},
            catch: (cause) =>
              new EncodeError({
                message: "Failed to encode API stream request",
                cause,
              }),
          })

          const response = yield* fetchResponse(config, endpoint, encoded)
          if (!response.ok) {
            const body = yield* readResponseBody(response)
            const domainError = endpoint.decodeError?.(response.status, body)
            if (domainError !== undefined) {
              return yield* domainError.pipe(Effect.flatMap((error) => Effect.fail(error)))
            }

            return yield* new UnexpectedStatusError({
              message: `Unexpected HTTP status ${response.status}`,
              status: response.status,
              body,
            })
          }

          if (response.body === null) {
            return yield* new RemoteProtocolError({
              message: "SSE response did not include a readable body",
            })
          }

          return Stream.fromAsyncIterable(
            decodeSseStream(response.body, endpoint),
            (cause) => cause as DomainError | ApiClientError,
          )
        }),
      ),
  {
    decode: decodeWithSchema,
    encode: encodeWithSchema,
  },
)

const fetchResponse = <Args, Success, DomainError>(
  config: FetchClientConfig,
  endpoint: UnaryHttpEndpoint<Args, Success, DomainError> | SseEndpoint<Args, Success, DomainError>,
  encoded: EncodedRequest,
): Effect.Effect<Response, NetworkError | TimeoutError | EncodeError> =>
  Effect.tryPromise({
    try: async (signal) => {
      const fetchImpl = config.fetch ?? globalThis.fetch
      if (fetchImpl === undefined) {
        throw new NetworkError({ message: "No fetch implementation is available" })
      }

      const timeout = makeTimeoutSignal(config.timeoutMs, signal)
      try {
        const init: RequestInit = {
          method: endpoint.method,
          headers: buildHeaders(config.headers, encoded),
          signal: timeout.signal,
        }
        const body = encodeBody(encoded.body)
        if (body !== undefined) {
          init.body = body
        }
        return await fetchImpl(buildUrl(config.baseUrl, endpoint.path, encoded), init)
      } finally {
        timeout.clear()
      }
    },
    catch: (cause) =>
      cause instanceof TimeoutError
        ? cause
        : cause instanceof NetworkError
          ? cause
          : new NetworkError({
              message: "API request failed",
              cause,
            }),
  })

const readResponseBody = (
  response: Response,
): Effect.Effect<unknown, RemoteProtocolError> =>
  Effect.tryPromise({
    try: async () => {
      if (response.status === 204) {
        return undefined
      }

      const text = await response.text()
      if (text.length === 0) {
        return undefined
      }

      try {
        return JSON.parse(text) as unknown
      } catch (cause) {
        throw new RemoteProtocolError({
          message: "Response body is not valid JSON",
          body: text,
        })
      }
    },
    catch: (cause) =>
      cause instanceof RemoteProtocolError
        ? cause
        : new RemoteProtocolError({
            message: "Failed to read API response body",
            body: cause,
          }),
  })

interface SseFrame {
  readonly event: string
  readonly data: string
}

interface ApiErrorFrame {
  readonly status: number
  readonly body: unknown
}

async function* decodeSseStream<Args, Success, DomainError>(
  body: ReadableStream<Uint8Array>,
  endpoint: SseEndpoint<Args, Success, DomainError>,
): AsyncIterable<Success> {
  const reader = body.getReader()
  const decoder = new TextDecoder()
  let buffer = ""

  try {
    while (true) {
      const read = await readSseChunk(reader)
      if (read.done) {
        buffer += decoder.decode()
        break
      }

      buffer += decoder.decode(read.value, { stream: true })
      const frames = drainSseFrames(buffer)
      buffer = frames.remainder
      for (const frame of frames.frames) {
        const decoded = await decodeSseFrame(frame, endpoint)
        if (decoded !== undefined) {
          yield decoded
        }
      }
    }

    const finalFrame = parseSseFrame(buffer)
    if (finalFrame !== undefined) {
      const decoded = await decodeSseFrame(finalFrame, endpoint)
      if (decoded !== undefined) {
        yield decoded
      }
    }
  } finally {
    reader.releaseLock()
  }
}

const readSseChunk = (
  reader: ReadableStreamDefaultReader<Uint8Array>,
): Promise<ReadableStreamReadResult<Uint8Array>> =>
  reader.read().catch((cause) => {
    throw new NetworkError({
      message: "SSE stream failed while reading response body",
      cause,
    })
  })

const decodeSseFrame = async <Args, Success, DomainError>(
  frame: SseFrame,
  endpoint: SseEndpoint<Args, Success, DomainError>,
): Promise<Success | undefined> => {
  if (frame.data.length === 0) {
    return undefined
  }

  if (frame.event === "api-error") {
    const envelope = parseJson(frame.data)
    if (!isApiErrorFrame(envelope)) {
      throw new RemoteProtocolError({
        message: "SSE error event did not include status and body",
        body: envelope,
      })
    }

    const domainError = endpoint.decodeError?.(envelope.status, envelope.body)
    if (domainError !== undefined) {
      throw await Effect.runPromise(domainError)
    }

    throw new UnexpectedStatusError({
      message: `Unexpected SSE error status ${envelope.status}`,
      status: envelope.status,
      body: envelope.body,
    })
  }

  return await Effect.runPromise(endpoint.decodeSuccess(parseJson(frame.data)))
}

const drainSseFrames = (
  input: string,
): { readonly frames: ReadonlyArray<SseFrame>; readonly remainder: string } => {
  const normalized = input.replace(/\r\n/g, "\n")
  const parts = normalized.split("\n\n")
  const remainder = parts.pop() ?? ""
  return {
    frames: parts.flatMap((part) => {
      const frame = parseSseFrame(part)
      return frame === undefined ? [] : [frame]
    }),
    remainder,
  }
}

const parseSseFrame = (input: string): SseFrame | undefined => {
  const lines = input.replace(/\r\n/g, "\n").split("\n")
  let event = "message"
  const data: Array<string> = []

  for (const line of lines) {
    if (line.length === 0 || line.startsWith(":")) {
      continue
    }

    const separator = line.indexOf(":")
    const field = separator === -1 ? line : line.slice(0, separator)
    const rawValue = separator === -1 ? "" : line.slice(separator + 1)
    const value = rawValue.startsWith(" ") ? rawValue.slice(1) : rawValue

    if (field === "event") {
      event = value
    } else if (field === "data") {
      data.push(value)
    }
  }

  if (data.length === 0) {
    return undefined
  }

  return {
    event,
    data: data.join("\n"),
  }
}

const parseJson = (text: string): unknown => {
  try {
    return JSON.parse(text) as unknown
  } catch (cause) {
    throw new RemoteProtocolError({
      message: "SSE event data is not valid JSON",
      body: text,
    })
  }
}

const isApiErrorFrame = (input: unknown): input is ApiErrorFrame =>
  typeof input === "object" &&
  input !== null &&
  typeof (input as { readonly status?: unknown }).status === "number" &&
  "body" in input

const buildUrl = (
  baseUrl: string,
  pathPattern: string,
  request: EncodedRequest,
): string => {
  const base = baseUrl.endsWith("/") ? baseUrl.slice(0, -1) : baseUrl
  const path = pathPattern.replace(/[:{]([A-Za-z_][A-Za-z0-9_]*)}?/g, (match, key) => {
    const value = request.path?.[String(key)]
    return value === undefined || value === null ? match : encodeURIComponent(String(value))
  })
  const combined = `${base}${path.startsWith("/") ? path : `/${path}`}`
  const isAbsolute = /^[A-Za-z][A-Za-z0-9+.-]*:/.test(combined)
  const url = new URL(combined, isAbsolute ? undefined : "http://rust-ts-integration.local")

  for (const [key, value] of Object.entries(request.query ?? {})) {
    const values = Array.isArray(value) ? value : [value]
    for (const item of values) {
      if (item !== undefined && item !== null) {
        url.searchParams.append(key, String(item))
      }
    }
  }

  if (isAbsolute) {
    return url.toString()
  }

  return `${url.pathname}${url.search}${url.hash}`
}

const buildHeaders = (
  configHeaders: HeadersInit | undefined,
  request: EncodedRequest,
): Headers => {
  const headers = new Headers(configHeaders)
  const requestHeaders = new Headers(request.headers)
  requestHeaders.forEach((value, key) => headers.set(key, value))
  if (request.body !== undefined && request.body !== null && !headers.has("content-type")) {
    headers.set("content-type", "application/json")
  }
  return headers
}

const encodeBody = (body: unknown): BodyInit | undefined => {
  if (body === undefined || body === null) {
    return undefined
  }
  if (typeof body === "string" || body instanceof FormData || body instanceof Blob) {
    return body
  }
  return JSON.stringify(body, encodeJsonValue)
}

const encodeJsonValue = (_key: string, value: unknown): unknown =>
  typeof value === "bigint" ? value.toString() : value

const makeTimeoutSignal = (
  timeoutMs: number | undefined,
  parent: AbortSignal,
): { readonly signal: AbortSignal; readonly clear: () => void } => {
  const controller = new AbortController()
  let timeoutId: ReturnType<typeof setTimeout> | undefined

  const abortFromParent = () => controller.abort(parent.reason)
  parent.addEventListener("abort", abortFromParent, { once: true })

  if (timeoutMs !== undefined) {
    timeoutId = setTimeout(() => {
      controller.abort(
        new TimeoutError({
          message: `API request timed out after ${timeoutMs}ms`,
          timeoutMs,
        }),
      )
    }, timeoutMs)
  }

  return {
    signal: controller.signal,
    clear: () => {
      parent.removeEventListener("abort", abortFromParent)
      if (timeoutId !== undefined) {
        clearTimeout(timeoutId)
      }
      if (controller.signal.reason instanceof TimeoutError) {
        throw controller.signal.reason
      }
    },
  }
}
