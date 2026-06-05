export interface ApiClientError {
  readonly _tag: "ApiClientError"
  readonly message: string
}

export const makeApiClientError = (message: string): ApiClientError => ({
  _tag: "ApiClientError",
  message,
})

