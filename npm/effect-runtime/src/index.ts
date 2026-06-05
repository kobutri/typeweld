import { Effect, Schema } from "effect"

export type HttpMethod = "DELETE" | "GET" | "PATCH" | "POST" | "PUT"

export type RequestValue = string | number | boolean | null | undefined

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
  },
)

const fetchResponse = <Args, Success, DomainError>(
  config: FetchClientConfig,
  endpoint: UnaryHttpEndpoint<Args, Success, DomainError>,
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
  return JSON.stringify(body)
}

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
