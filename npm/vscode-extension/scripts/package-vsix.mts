import { spawnSync } from "node:child_process"
import { mkdirSync, readFileSync, rmSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const args = process.argv.slice(2)
const scriptDir = dirname(fileURLToPath(import.meta.url))
const extensionRoot = resolve(scriptDir, "..")
const manifest = JSON.parse(
  readFileSync(join(extensionRoot, "package.json"), "utf8"),
)
const { packageArgs, outPath } = splitArgs(args)
const target = parseTarget(packageArgs)
const finalOutPath = outPath ?? defaultVsixPath(manifest, target)
const temporaryVsixPath = join(
  extensionRoot,
  "out",
  `.${manifest.name}-${process.pid}.unpatched.vsix`,
)
const vsceCommand = process.platform === "win32" ? "vsce.cmd" : "vsce"

mkdirSync(join(extensionRoot, "out"), { recursive: true })
rmSync(temporaryVsixPath, { force: true })

try {
  run(vsceCommand, [
    "package",
    "--no-dependencies",
    "--out",
    temporaryVsixPath,
    ...packageArgs,
  ])
  run("tsx", [
    "scripts/patch-vsix-typescript-plugin.mts",
    "--vsix",
    temporaryVsixPath,
    "--out",
    finalOutPath,
  ])
} finally {
  rmSync(temporaryVsixPath, { force: true })
}

function splitArgs(args: readonly string[]): {
  readonly packageArgs: string[]
  readonly outPath: string | undefined
} {
  const packageArgs: string[] = []
  let outPath: string | undefined

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
      outPath = value
      index += 1
      continue
    }

    if (arg.startsWith("--out=")) {
      outPath = arg.slice("--out=".length)
      continue
    }

    packageArgs.push(arg)
  }

  return { packageArgs, outPath }
}

function parseTarget(args: readonly string[]): string | undefined {
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index]
    if (arg === "--target" || arg === "-t") {
      return args[index + 1]
    }
    if (arg?.startsWith("--target=")) {
      return arg.slice("--target=".length)
    }
  }
  return undefined
}

function defaultVsixPath(
  manifest: { readonly name: string; readonly version: string },
  target: string | undefined,
): string {
  const targetSegment = target === undefined ? "" : `-${target}`
  return `${manifest.name}${targetSegment}-${manifest.version}.vsix`
}

function run(command: string, args: readonly string[]): void {
  const result = spawnSync(command, args, {
    cwd: extensionRoot,
    shell: process.platform === "win32",
    stdio: "inherit",
  })

  if (result.error !== undefined) {
    throw result.error
  }

  if (result.status !== 0) {
    process.exit(result.status ?? 1)
  }
}
