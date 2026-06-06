import { execFileSync } from "node:child_process"
import { copyFileSync, cpSync, mkdirSync, readFileSync, rmSync, statSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const scriptDir = dirname(fileURLToPath(import.meta.url))
const extensionRoot = resolve(scriptDir, "..")
const manifest = JSON.parse(
  readFileSync(join(extensionRoot, "package.json"), "utf8"),
)
const vsixPath = join(extensionRoot, `${manifest.name}-${manifest.version}.vsix`)
const outPath = parseOutPath(process.argv.slice(2))
const pluginSource = join(extensionRoot, "typescript-plugin")
const stagingRoot = join(extensionRoot, "out", "vsix-typescript-plugin")
const pluginTarget = join(
  stagingRoot,
  "extension",
  "node_modules",
  "@rust-ts-integration",
  "typescript-plugin",
)

assertFile(vsixPath, "VSIX")
assertDirectory(pluginSource, "TypeScript server plugin")

rmSync(stagingRoot, { force: true, recursive: true })
mkdirSync(dirname(pluginTarget), { recursive: true })
cpSync(pluginSource, pluginTarget, { recursive: true })
execFileSync("zip", ["-qr", vsixPath, "extension/node_modules"], {
  cwd: stagingRoot,
  stdio: "inherit",
})

console.log(`Patched TypeScript server plugin into ${vsixPath}`)

if (outPath !== undefined) {
  mkdirSync(dirname(outPath), { recursive: true })
  copyFileSync(vsixPath, outPath)
  console.log(`Wrote patched VSIX to ${outPath}`)
}

function parseOutPath(args: readonly string[]): string | undefined {
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index]
    if (arg === undefined) {
      continue
    }
    if (arg === "--out") {
      const value = args[index + 1]
      if (value === undefined || value.startsWith("--")) {
        throw new Error("--out requires a path")
      }
      return resolve(extensionRoot, value)
    }
    if (arg.startsWith("--out=")) {
      return resolve(extensionRoot, arg.slice("--out=".length))
    }
  }
  return undefined
}

function assertFile(path: string, label: string): void {
  try {
    if (statSync(path).isFile()) {
      return
    }
  } catch {
  }
  throw new Error(`Missing ${label}: ${path}`)
}

function assertDirectory(path: string, label: string): void {
  try {
    if (statSync(path).isDirectory()) {
      return
    }
  } catch {
  }
  throw new Error(`Missing ${label}: ${path}`)
}
