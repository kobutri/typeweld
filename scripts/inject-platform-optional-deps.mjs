// Injects @typeweld/cli-<platform> optionalDependencies into the launcher's
// package.json at release time, pinned to the launcher's own version.
//
// The committed manifest deliberately omits them: a release's platform
// packages only exist on the registry once that same release publishes them,
// so a committed pin can never be reflected in package-lock.json and strict
// `npm ci` (npm >= 11) fails the lockfile sync check.
//
// Usage: node scripts/inject-platform-optional-deps.mjs

import { readFileSync, writeFileSync } from "node:fs"
import { join } from "node:path"

const platforms = ["linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64", "win32-x64"]

const manifestPath = join("npm", "typeweld", "package.json")
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"))

manifest.optionalDependencies = Object.fromEntries(
  platforms.map((platform) => [`@typeweld/cli-${platform}`, manifest.version]),
)

writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`)
console.log(`injected @typeweld/cli-*@${manifest.version} optionalDependencies into ${manifestPath}`)
