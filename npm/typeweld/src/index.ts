#!/usr/bin/env node

import { spawn } from "node:child_process"
import { accessSync, constants, realpathSync, statSync } from "node:fs"
import { createRequire } from "node:module"
import { delimiter, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const executableName =
  process.platform === "win32" ? "typeweld.exe" : "typeweld"
const platformTarget = `${process.platform}-${process.arch}`
const platformBinarySpecifier = `@typeweld/cli-${platformTarget}/bin/${platformTarget}/${executableName}`
const launcherFile = fileURLToPath(import.meta.url)
const defaultPackageRoot = resolve(launcherFile, "..", "..")
const buildHints = [
  "Build the CLI with:",
  "  cargo build -p typeweld-cli --bin typeweld",
  "",
  "Or set TYPEWELD_BINARY to the absolute path of a working typeweld binary.",
]

export type ResolveOptions = {
  env?: NodeJS.ProcessEnv
  cwd?: string
  packageRoot?: string
}

export class TypeweldResolveError extends Error {
  readonly checked: readonly string[]

  constructor(message: string, checked: readonly string[]) {
    super(message)
    this.name = "TypeweldResolveError"
    this.checked = checked
  }
}

export function resolveTypeweldBinary(options: ResolveOptions = {}): string {
  const env = options.env ?? process.env
  const cwd = resolve(options.cwd ?? process.cwd())
  const packageRoot = resolve(options.packageRoot ?? defaultPackageRoot)
  const checked: string[] = []

  const override = env.TYPEWELD_BINARY?.trim()
  if (override !== undefined && override !== "") {
    const path = resolve(cwd, override)
    if (isExecutable(path)) {
      return path
    }
    throw new TypeweldResolveError(
      [
        `TYPEWELD_BINARY points to \`${path}\`, but it does not exist or is not executable.`,
        "",
        ...buildHints,
      ].join("\n"),
      [`TYPEWELD_BINARY: ${path} (missing or not executable)`],
    )
  }
  checked.push("TYPEWELD_BINARY: not set")

  try {
    const requireFromPackage = createRequire(join(packageRoot, "package.json"))
    return requireFromPackage.resolve(platformBinarySpecifier)
  } catch {
    checked.push(`platform package: ${platformBinarySpecifier} is not installed`)
  }

  const pathDirs = pathEntries(env)
  for (const directory of pathDirs) {
    const candidate = resolve(directory, executableName)
    if (isExecutable(candidate) && !isSameFile(candidate, launcherFile)) {
      return candidate
    }
  }
  checked.push(
    `PATH: no \`${executableName}\` found in ${pathDirs.length} entries`,
  )

  for (const profile of ["debug", "release"]) {
    const candidate = resolve(
      packageRoot,
      "..",
      "..",
      "target",
      profile,
      executableName,
    )
    if (isExecutable(candidate)) {
      return candidate
    }
    checked.push(`local cargo build: ${candidate} (missing or not executable)`)
  }

  throw new TypeweldResolveError(
    [
      "typeweld launcher could not find the typeweld binary.",
      "",
      "Checked:",
      ...checked.map((line) => `  - ${line}`),
      "",
      ...buildHints,
    ].join("\n"),
    checked,
  )
}

export function main(): void {
  let binary: string
  try {
    binary = resolveTypeweldBinary()
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error))
    process.exit(1)
  }

  const child = spawn(binary, process.argv.slice(2), {
    stdio: "inherit",
    windowsHide: true,
  })
  for (const signal of ["SIGINT", "SIGTERM"] as const) {
    process.once(signal, () => child.kill(signal))
  }
  child.once("error", (error) => {
    console.error(`failed to start \`${binary}\`: ${error.message}`)
    process.exit(1)
  })
  child.once("exit", (code, signal) => {
    if (signal !== null) {
      process.kill(process.pid, signal)
      setTimeout(() => process.exit(1), 250).unref()
      return
    }
    process.exit(code ?? 1)
  })
}

function pathEntries(env: NodeJS.ProcessEnv): readonly string[] {
  const pathKey = Object.keys(env).find((key) => key.toLowerCase() === "path")
  const pathValue = pathKey === undefined ? undefined : env[pathKey]
  return pathValue === undefined || pathValue === ""
    ? []
    : pathValue.split(delimiter).filter((entry) => entry !== "")
}

function isExecutable(path: string): boolean {
  try {
    if (!statSync(path).isFile()) {
      return false
    }
    accessSync(
      path,
      process.platform === "win32" ? constants.F_OK : constants.X_OK,
    )
    return true
  } catch {
    return false
  }
}

function isSameFile(left: string, right: string): boolean {
  try {
    return realpathSync(left) === realpathSync(right)
  } catch {
    return false
  }
}

const invokedFile = process.argv[1]
if (invokedFile !== undefined && isSameFile(invokedFile, launcherFile)) {
  main()
}
