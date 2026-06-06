import { chmodSync, copyFileSync, mkdirSync, rmSync, statSync, writeFileSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

import * as esbuild from "esbuild"

const scriptDir = dirname(fileURLToPath(import.meta.url))
const extensionRoot = resolve(scriptDir, "..")
const npmRoot = resolve(extensionRoot, "..")
const repositoryRoot = resolve(npmRoot, "..")
const languageServerBinaryName = process.platform === "win32" ? "typeweld-ls.exe" : "typeweld-ls"
const typeweldBinaryName = process.platform === "win32" ? "typeweld.exe" : "typeweld"

type PreparePackageOptions = {
  readonly typeweldBinary?: string
  readonly binary?: string
  readonly platformDir?: string
}

const options = parseArgs(process.argv.slice(2))
const platformDir = options.platformDir ?? `${process.platform}-${process.arch}`
const binaryPath = resolve(
  extensionRoot,
  options.binary ?? process.env.TYPEWELD_LS_BINARY ?? defaultLanguageServerBinaryPath(),
)
const configuredTypeweldBinary =
  options.typeweldBinary ?? process.env.TYPEWELD_BINARY ?? process.env.API_CLI_BINARY
const typeweldBinaryPath =
  configuredTypeweldBinary !== undefined
    ? resolve(extensionRoot, configuredTypeweldBinary)
    : defaultTypeweldBinaryPath()

assertFile(binaryPath, "typeweld-ls binary", "Build typeweld-ls first or pass --binary.")

const launcherSource = resolve(
  npmRoot,
  "language-server-wrapper",
  "src",
  "index.ts",
)
const launcherTarget = join(extensionRoot, "server", "index.js")
const launcherPackageTarget = join(extensionRoot, "server", "package.json")
const binaryTarget = join(extensionRoot, "bin", platformDir, languageServerBinaryName)
const typeweldBinaryTarget = join(extensionRoot, "bin", platformDir, typeweldBinaryName)
const typescriptPluginSource = join(extensionRoot, "typescript-plugin")
const typescriptPluginTarget = join(
  extensionRoot,
  "node_modules",
  "@typeweld",
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
const copiedTypeweldBinary = maybeCopyTypeweldBinary(typeweldBinaryPath, typeweldBinaryTarget)
await buildTypescriptPlugin(typescriptPluginSource, typescriptPluginTarget)

if (process.platform !== "win32") {
  chmodSync(launcherTarget, 0o755)
  chmodSync(binaryTarget, 0o755)
  if (copiedTypeweldBinary) {
    chmodSync(typeweldBinaryTarget, 0o755)
  }
}

console.log(`Prepared typeweld-ls launcher at ${launcherTarget}`)
console.log(`Prepared ${platformDir} typeweld-ls binary at ${binaryTarget}`)
console.log(`Prepared TypeScript server plugin at ${typescriptPluginTarget}`)
if (copiedTypeweldBinary) {
  console.log(`Prepared ${platformDir} typeweld binary at ${typeweldBinaryTarget}`)
}

function defaultLanguageServerBinaryPath(): string {
  return join(repositoryRoot, "target", "release", languageServerBinaryName)
}

function defaultTypeweldBinaryPath(): string {
  return join(repositoryRoot, "target", "release", typeweldBinaryName)
}

function maybeCopyTypeweldBinary(source: string, target: string): boolean {
  try {
    if (!statSync(source).isFile()) {
      return false
    }
  } catch {
    if (
      options.typeweldBinary !== undefined ||
      process.env.TYPEWELD_BINARY !== undefined ||
      process.env.API_CLI_BINARY !== undefined
    ) {
      throw new Error(`Missing typeweld binary: ${source}. Build typeweld first or omit --typeweld-binary.`)
    }
    return false
  }

  copyFileSync(source, target)
  return true
}

async function buildTypescriptPlugin(source: string, target: string): Promise<void> {
  const entry = join(source, "index.ts")
  const manifest = join(source, "package.json")
  assertFile(entry, "TypeScript server plugin source")
  assertFile(manifest, "TypeScript server plugin manifest")

  rmSync(target, { force: true, recursive: true })
  mkdirSync(target, { recursive: true })
  copyFileSync(manifest, join(target, "package.json"))
  await esbuild.build({
    bundle: false,
    entryPoints: [entry],
    format: "cjs",
    legalComments: "none",
    logLevel: "silent",
    outfile: join(target, "index.js"),
    platform: "node",
    target: "node20",
  })
}

function assertFile(path: string, label: string, hint = ""): void {
  try {
    if (!statSync(path).isFile()) {
      throw new Error("not a file")
    }
  } catch (error) {
    const suffix = hint === "" ? "" : ` ${hint}`
    throw new Error(`Missing ${label}: ${path}.${suffix}`)
  }
}

function parseArgs(args: readonly string[]): PreparePackageOptions {
  const parsed: {
    typeweldBinary?: string
    binary?: string
    platformDir?: string
  } = {}
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index]
    if (arg === "--binary") {
      parsed.binary = requireValue(args, ++index, arg)
    } else if (arg === "--typeweld-binary" || arg === "--api-binary") {
      parsed.typeweldBinary = requireValue(args, ++index, arg)
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
