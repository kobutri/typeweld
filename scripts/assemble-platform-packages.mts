// Assembles @typeweld/cli-<platform> npm packages from CI binary artifacts.
//
// Usage: node scripts/assemble-platform-packages.mts <artifacts-dir> <out-dir>
// Expects <artifacts-dir>/typeweld-<platform>/<platform>/typeweld[.exe]
// as uploaded by the release workflow's build-binaries job.

import { chmodSync, cpSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs"
import { join } from "node:path"

const [artifactsDir, outDir] = process.argv.slice(2)
if (!artifactsDir || !outDir) {
  console.error("usage: assemble-platform-packages.mts <artifacts-dir> <out-dir>")
  process.exit(1)
}

const launcher = JSON.parse(
  readFileSync(join("npm", "typeweld", "package.json"), "utf8"),
) as { version: string }

const platforms: Record<string, { os: string; cpu: string }> = {
  "linux-x64": { os: "linux", cpu: "x64" },
  "linux-arm64": { os: "linux", cpu: "arm64" },
  "darwin-x64": { os: "darwin", cpu: "x64" },
  "darwin-arm64": { os: "darwin", cpu: "arm64" },
  "win32-x64": { os: "win32", cpu: "x64" },
}

let assembled = 0
for (const entry of readdirSync(artifactsDir)) {
  const platform = entry.replace(/^typeweld-/, "")
  const spec = platforms[platform]
  if (!spec) continue

  const packageDir = join(outDir, `cli-${platform}`)
  const binDir = join(packageDir, "bin", platform)
  mkdirSync(binDir, { recursive: true })
  cpSync(join(artifactsDir, entry, platform), binDir, { recursive: true })
  for (const file of readdirSync(binDir)) {
    if (!file.endsWith(".exe")) chmodSync(join(binDir, file), 0o755)
  }

  writeFileSync(
    join(packageDir, "package.json"),
    `${JSON.stringify(
      {
        name: `@typeweld/cli-${platform}`,
        version: launcher.version,
        description: `typeweld CLI binary for ${platform}`,
        license: "MIT OR Apache-2.0",
        repository: { type: "git", url: "git+https://github.com/kobutri/typeweld.git" },
        os: [spec.os],
        cpu: [spec.cpu],
        files: ["bin"],
      },
      null,
      2,
    )}\n`,
  )
  assembled += 1
  console.log(`assembled @typeweld/cli-${platform}@${launcher.version}`)
}

if (assembled === 0) {
  console.error(`no platform artifacts found in ${artifactsDir}`)
  process.exit(1)
}
