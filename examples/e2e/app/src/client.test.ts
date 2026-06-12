import { describe, expect, it } from "vitest"
import { Effect } from "effect"
import {
  avatarRoundTrip,
  createAndFetch,
  fetchMissing,
  firstEvents,
  search,
} from "./client"

describe("e2e round trip", () => {
  it("creates and fetches a user", async () => {
    const user = await Effect.runPromise(createAndFetch("Grace"))
    expect(user.displayName).toBe("Grace")
    expect(user.bio ?? null).toBeNull()
  })

  it("lists users with query params", async () => {
    await Effect.runPromise(createAndFetch("Lin"))
    const users = await Effect.runPromise(search("Lin", 10))
    expect(users.length).toBeGreaterThan(0)
    expect(users[0]?.displayName).toContain("Lin")
  })

  it("streams server-sent events", async () => {
    const events = await Effect.runPromise(firstEvents(2))
    expect(events).toHaveLength(2)
    expect(events[0]?._tag).toBe("Created")
    expect(events[1]?._tag).toBe("Renamed")
  })

  it("uploads and downloads binary data", async () => {
    const user = await Effect.runPromise(createAndFetch("Avatar"))
    const payload = new TextEncoder().encode("png-bytes")
    const downloaded = await Effect.runPromise(avatarRoundTrip(user.id, payload))
    expect(new TextDecoder().decode(downloaded)).toBe("png-bytes")
  })

  it("decodes domain errors into catchable tagged classes", async () => {
    const result = await Effect.runPromise(fetchMissing(99999))
    expect(result).toBe("missing:99999")
  })
})
