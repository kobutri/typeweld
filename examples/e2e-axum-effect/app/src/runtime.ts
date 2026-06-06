import { Effect, Schema } from "@rust-ts-integration/effect-runtime/compat"
import { User } from "@workspace/e2e-api"

import { program } from "./client.js"

export const runAgainstServer = async (baseUrl: string) => {
  const rawUserResponse = await fetch(`${baseUrl}/users/1`)
  if (!rawUserResponse.ok) {
    throw new Error(`direct schema fetch failed with ${rawUserResponse.status}`)
  }
  const decodedUser = Schema.decodeUnknownSync(User)(await rawUserResponse.json())
  expectEqual(decodedUser.displayName, "Ada Lovelace", "direct User schema decode")

  const result = await Effect.runPromise(program(baseUrl))

  expectEqual(result.displayName, "Ada Lovelace", "displayName field")
  expectEqual(result.guestName, "Guest", "UserNotFound catchTag result")
  expectEqual(result.createdName, "Grace Hopper", "created user field")
  expectEqual(result.conflictName, "Root", "DisplayNameTaken catchTag field")
  expectArrayEqual(result.eventKinds, ["connected", "created"], "SSE event kinds")
}

const expectEqual = (actual: string, expected: string, label: string) => {
  if (actual !== expected) {
    throw new Error(`${label}: expected ${expected}, got ${actual}`)
  }
}

const expectArrayEqual = (
  actual: ReadonlyArray<string>,
  expected: ReadonlyArray<string>,
  label: string,
) => {
  if (
    actual.length !== expected.length ||
    actual.some((value, index) => value !== expected[index])
  ) {
    throw new Error(
      `${label}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    )
  }
}
