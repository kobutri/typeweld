import {
  chmodSync,
  copyFileSync,
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { spawnSync } from "node:child_process"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const scriptDir = dirname(fileURLToPath(import.meta.url))
const extensionRoot = resolve(scriptDir, "..")
const repositoryRoot = resolve(extensionRoot, "..", "..")

type PackageVsixOptions = {
  readonly out?: string
  readonly rustTarget?: string
  readonly target: string
}

type ExtensionManifest = {
  readonly name: string
  readonly version: string
  readonly [key: string]: unknown
}

const options = parseArgs(process.argv.slice(2))
const extensionManifest = JSON.parse(
  readFileSync(join(extensionRoot, "package.json"), "utf8"),
) as ExtensionManifest
const outputPath = options.out ?? join(
  extensionRoot,
  `${extensionManifest.name}-${extensionManifest.version}.vsix`,
)
const binaryDirectory = join(extensionRoot, "bin", options.target)
const releaseDirectory = options.rustTarget === undefined
  ? join(repositoryRoot, "target", "release")
  : join(repositoryRoot, "target", options.rustTarget, "release")

run("npm", ["run", "build"], extensionRoot)

run("cargo", [
  "build",
  "-p",
  "typeweld-ls",
  "-p",
  "typeweld-cli",
  "--bins",
  "--release",
  ...(options.rustTarget === undefined ? [] : ["--target", options.rustTarget]),
], repositoryRoot)

mkdirSync(binaryDirectory, { recursive: true })
copyBinary("typeweld-ls")
copyBinary("typeweld")

mkdirSync(dirname(outputPath), { recursive: true })

const stagingRoot = mkdtempSync(join(tmpdir(), "typeweld-vscode-"))
try {
  stageExtension(stagingRoot, extensionManifest)
  run("vsce", [
    "package",
    "--target",
    options.target,
    "--dependencies",
    "--follow-symlinks",
    "--out",
    outputPath,
  ], stagingRoot)
} finally {
  rmSync(stagingRoot, { force: true, recursive: true })
}

function copyBinary(baseName: "typeweld-ls" | "typeweld"): void {
  const name = binaryName(baseName, options.target)
  const source = join(releaseDirectory, name)
  const target = join(binaryDirectory, name)
  copyFileSync(source, target)
  if (!options.target.startsWith("win32-")) {
    chmodSync(target, 0o755)
  }
}

function stageExtension(
  stagingRoot: string,
  manifest: ExtensionManifest,
): void {
  const stagedManifest = {
    ...manifest,
    dependencies: {
      "@typeweld/typescript-plugin": manifest.version,
    },
    devDependencies: undefined,
    scripts: undefined,
  }

  writeFileSync(
    join(stagingRoot, "package.json"),
    `${JSON.stringify(stagedManifest, null, 2)}\n`,
  )
  copyDirectory("bin", stagingRoot)
  copyDirectory("dist", stagingRoot)
  copyFileSync(join(extensionRoot, "README.md"), join(stagingRoot, "README.md"))

  const pluginRoot = join(
    stagingRoot,
    "node_modules",
    "@typeweld",
    "typescript-plugin",
  )
  mkdirSync(pluginRoot, { recursive: true })
  copyFileSync(
    join(extensionRoot, "typescript-plugin", "package.json"),
    join(pluginRoot, "package.json"),
  )
  cpSync(
    join(extensionRoot, "typescript-plugin", "dist"),
    join(pluginRoot, "dist"),
    { recursive: true },
  )
}

function copyDirectory(name: string, targetRoot: string): void {
  cpSync(join(extensionRoot, name), join(targetRoot, name), {
    recursive: true,
  })
}

function parseArgs(args: readonly string[]): PackageVsixOptions {
  const parsed: {
    out?: string
    rustTarget?: string
    target?: string
  } = {}

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index]
    if (arg === undefined) {
      continue
    }
    if (arg === "--target") {
      parsed.target = requireValue(args, ++index, arg)
    } else if (arg.startsWith("--target=")) {
      parsed.target = arg.slice("--target=".length)
    } else if (arg === "--rust-target") {
      parsed.rustTarget = requireValue(args, ++index, arg)
    } else if (arg.startsWith("--rust-target=")) {
      parsed.rustTarget = arg.slice("--rust-target=".length)
    } else if (arg === "--out" || arg === "-o") {
      parsed.out = resolve(extensionRoot, requireValue(args, ++index, arg))
    } else if (arg.startsWith("--out=")) {
      parsed.out = resolve(extensionRoot, arg.slice("--out=".length))
    } else {
      throw new Error(`Unknown argument: ${arg}`)
    }
  }

  const options: {
    out?: string
    rustTarget?: string
    target: string
  } = { target: parsed.target ?? nativeVsceTarget() }
  if (parsed.out !== undefined) {
    options.out = parsed.out
  }
  if (parsed.rustTarget !== undefined) {
    options.rustTarget = parsed.rustTarget
  }
  return options
}

function requireValue(args: readonly string[], index: number, flag: string): string {
  const value = args[index]
  if (value === undefined || value.startsWith("-")) {
    throw new Error(`${flag} requires a value`)
  }
  return value
}

function nativeVsceTarget(): string {
  if (process.platform === "darwin") {
    return process.arch === "arm64" ? "darwin-arm64" : "darwin-x64"
  }
  if (process.platform === "win32") {
    return process.arch === "arm64" ? "win32-arm64" : "win32-x64"
  }
  if (process.platform === "linux") {
    if (process.arch === "arm64") {
      return "linux-arm64"
    }
    if (process.arch === "arm") {
      return "linux-armhf"
    }
    return "linux-x64"
  }
  throw new Error(`Unsupported VS Code extension target: ${process.platform}-${process.arch}`)
}

function binaryName(baseName: string, target: string): string {
  return target.startsWith("win32-") ? `${baseName}.exe` : baseName
}

function run(command: string, args: readonly string[], cwd: string): void {
  const result = spawnSync(command, args, {
    cwd,
    env: process.env,
    shell: process.platform === "win32",
    stdio: "inherit",
  })
  if (result.error !== undefined) {
    throw result.error
  }
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} failed with exit code ${result.status ?? "unknown"}`)
  }
}
