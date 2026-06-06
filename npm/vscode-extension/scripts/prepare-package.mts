import { chmodSync, copyFileSync, cpSync, mkdirSync, rmSync, statSync, writeFileSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

import * as esbuild from "esbuild"

const scriptDir = dirname(fileURLToPath(import.meta.url))
const extensionRoot = resolve(scriptDir, "..")
const npmRoot = resolve(extensionRoot, "..")
const repositoryRoot = resolve(npmRoot, "..")
const languageServerBinaryName = process.platform === "win32" ? "api-ls.exe" : "api-ls"
const apiBinaryName = process.platform === "win32" ? "api.exe" : "api"

type PreparePackageOptions = {
  readonly apiBinary?: string
  readonly binary?: string
  readonly platformDir?: string
}

const options = parseArgs(process.argv.slice(2))
const platformDir = options.platformDir ?? `${process.platform}-${process.arch}`
const binaryPath = resolve(
  extensionRoot,
  options.binary ?? process.env.API_LS_BINARY ?? defaultLanguageServerBinaryPath(),
)
const configuredApiBinary = options.apiBinary ?? process.env.API_CLI_BINARY
const apiBinaryPath =
  configuredApiBinary !== undefined
    ? resolve(extensionRoot, configuredApiBinary)
    : defaultApiBinaryPath()

assertFile(binaryPath, "api-ls binary")

const launcherSource = resolve(
  npmRoot,
  "language-server-wrapper",
  "src",
  "index.ts",
)
const launcherTarget = join(extensionRoot, "server", "index.js")
const launcherPackageTarget = join(extensionRoot, "server", "package.json")
const binaryTarget = join(extensionRoot, "bin", platformDir, languageServerBinaryName)
const apiBinaryTarget = join(extensionRoot, "bin", platformDir, apiBinaryName)
const typescriptPluginSource = join(extensionRoot, "typescript-plugin")
const typescriptPluginTarget = join(
  extensionRoot,
  "node_modules",
  "@rust-ts-integration",
  "typescript-plugin",
)

mkdirSync(dirname(launcherTarget), { recursive: true })
mkdirSync(dirname(binaryTarget), { recursive: true })
await esbuild.build({
  bundle: false,
  entryPoints: [launcherSource],
  format: "esm",
  legalComments: "none",
  logLevel: "silent",
  outfile: launcherTarget,
  platform: "node",
  target: "node20",
})
writeFileSync(launcherPackageTarget, `${JSON.stringify({ type: "module" }, null, 2)}\n`)
copyFileSync(binaryPath, binaryTarget)
const copiedApiBinary = maybeCopyApiBinary(apiBinaryPath, apiBinaryTarget)
copyDirectory(typescriptPluginSource, typescriptPluginTarget)

if (process.platform !== "win32") {
  chmodSync(launcherTarget, 0o755)
  chmodSync(binaryTarget, 0o755)
  if (copiedApiBinary) {
    chmodSync(apiBinaryTarget, 0o755)
  }
}

console.log(`Prepared api-ls launcher at ${launcherTarget}`)
console.log(`Prepared ${platformDir} api-ls binary at ${binaryTarget}`)
console.log(`Prepared TypeScript server plugin at ${typescriptPluginTarget}`)
if (copiedApiBinary) {
  console.log(`Prepared ${platformDir} api binary at ${apiBinaryTarget}`)
}

function defaultLanguageServerBinaryPath(): string {
  return join(repositoryRoot, "target", "release", languageServerBinaryName)
}

function defaultApiBinaryPath(): string {
  return join(repositoryRoot, "target", "release", apiBinaryName)
}

function maybeCopyApiBinary(source: string, target: string): boolean {
  try {
    if (!statSync(source).isFile()) {
      return false
    }
  } catch {
    if (options.apiBinary !== undefined || process.env.API_CLI_BINARY !== undefined) {
      throw new Error(`Missing api binary: ${source}. Build api first or omit --api-binary.`)
    }
    return false
  }

  copyFileSync(source, target)
  return true
}

function copyDirectory(source: string, target: string): void {
  rmSync(target, { force: true, recursive: true })
  mkdirSync(dirname(target), { recursive: true })
  cpSync(source, target, { recursive: true })
}

function assertFile(path: string, label: string): void {
  try {
    if (!statSync(path).isFile()) {
      throw new Error("not a file")
    }
  } catch (error) {
    throw new Error(`Missing ${label}: ${path}. Build api-ls first or pass --binary.`)
  }
}

function parseArgs(args: readonly string[]): PreparePackageOptions {
  const parsed: {
    apiBinary?: string
    binary?: string
    platformDir?: string
  } = {}
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index]
    if (arg === "--binary") {
      parsed.binary = requireValue(args, ++index, arg)
    } else if (arg === "--api-binary") {
      parsed.apiBinary = requireValue(args, ++index, arg)
    } else if (arg === "--platform-dir") {
      parsed.platformDir = requireValue(args, ++index, arg)
    } else {
      throw new Error(`Unknown argument: ${arg}`)
    }
  }
  return parsed
}

function requireValue(args: readonly string[], index: number, flag: string): string {
  const value = args[index]
  if (value === undefined || value.startsWith("--")) {
    throw new Error(`${flag} requires a value`)
  }
  return value
}
