import { Effect } from "@typeweld/effect-runtime/compat"
import { ServerApi, admin } from "@workspace/e2e-api"

export const loadAuditSummary = (baseUrl: string) =>
  Effect.gen(function* () {
    const audit = yield* admin.auditLog({})
    return `${audit.summary} at ${audit.generatedAt}`
  }).pipe(Effect.provide(ServerApi.layer({ baseUrl })))
