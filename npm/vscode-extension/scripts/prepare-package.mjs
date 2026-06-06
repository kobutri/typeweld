import { chmodSync, copyFileSync, mkdirSync, statSync, writeFileSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const scriptDir = dirname(fileURLToPath(import.meta.url))
const extensionRoot = resolve(scriptDir, "..")
const npmRoot = resolve(extensionRoot, "..")
const repositoryRoot = resolve(npmRoot, "..")
const languageServerBinaryName = process.platform === "win32" ? "api-ls.exe" : "api-ls"
const apiBinaryName = process.platform === "win32" ? "api.exe" : "api"

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
  "index.js",
)
const launcherTarget = join(extensionRoot, "server", "index.js")
const launcherPackageTarget = join(extensionRoot, "server", "package.json")
const binaryTarget = join(extensionRoot, "bin", platformDir, languageServerBinaryName)
const apiBinaryTarget = join(extensionRoot, "bin", platformDir, apiBinaryName)

mkdirSync(dirname(launcherTarget), { recursive: true })
mkdirSync(dirname(binaryTarget), { recursive: true })
copyFileSync(launcherSource, launcherTarget)
writeFileSync(launcherPackageTarget, `${JSON.stringify({ type: "module" }, null, 2)}\n`)
copyFileSync(binaryPath, binaryTarget)
const copiedApiBinary = maybeCopyApiBinary(apiBinaryPath, apiBinaryTarget)

if (process.platform !== "win32") {
  chmodSync(launcherTarget, 0o755)
  chmodSync(binaryTarget, 0o755)
  if (copiedApiBinary) {
    chmodSync(apiBinaryTarget, 0o755)
  }
}

console.log(`Prepared api-ls launcher at ${launcherTarget}`)
console.log(`Prepared ${platformDir} api-ls binary at ${binaryTarget}`)
if (copiedApiBinary) {
  console.log(`Prepared ${platformDir} api binary at ${apiBinaryTarget}`)
}

function defaultLanguageServerBinaryPath() {
  return join(repositoryRoot, "target", "release", languageServerBinaryName)
}

function defaultApiBinaryPath() {
  return join(repositoryRoot, "target", "release", apiBinaryName)
}

function maybeCopyApiBinary(source, target) {
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

function assertFile(path, label) {
  try {
    if (!statSync(path).isFile()) {
      throw new Error("not a file")
    }
  } catch (error) {
    throw new Error(`Missing ${label}: ${path}. Build api-ls first or pass --binary.`)
  }
}

function parseArgs(args) {
  const parsed = {}
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

function requireValue(args, index, flag) {
  const value = args[index]
  if (value === undefined || value.startsWith("--")) {
    throw new Error(`${flag} requires a value`)
  }
  return value
}
