#!/usr/bin/env node

import { spawn } from "node:child_process"
import {
  accessSync,
  constants,
  existsSync,
  realpathSync,
  statSync,
} from "node:fs"
import { delimiter, dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

export const binaryName = "typeweld"
export const binaryOverrideEnvVars = ["TYPEWELD_BINARY", "API_CLI_BINARY"]

const wrapperFile = fileURLToPath(import.meta.url)
const platformBinaryName = process.platform === "win32" ? `${binaryName}.exe` : binaryName

export class TypeweldWrapperError extends Error {
  constructor(message, checked) {
    super(message)
    this.name = "TypeweldWrapperError"
    this.checked = checked
  }
}

export function resolveTypeweldBinary(options = {}) {
  const env = options.env ?? process.env
  const cwd = resolve(options.cwd ?? process.cwd())
  const currentWrapperFile = resolve(options.wrapperFile ?? wrapperFile)
  const invocationFile = options.invocationFile
    ? resolve(options.invocationFile)
    : process.argv[1]
  const wrapperRealpaths = realpathSet([currentWrapperFile, invocationFile])
  const checked = []
  const override = resolveOverride(env, cwd)

  if (override !== undefined) {
    const status = executableStatus(override.path)
    checked.push(formatChecked(override.source, override.path, status.reason))
    if (status.ok) {
      return override.path
    }

    throw new TypeweldWrapperError(
      [
        `${override.source} points to \`${override.path}\`, but ${status.reason}.`,
        "",
        "Build the CLI with:",
        "  cargo build -p typeweld-cli --bin typeweld",
        "",
        "Or set TYPEWELD_BINARY to the absolute path of a working typeweld binary.",
      ].join("\n"),
      checked,
    )
  }

  for (const candidate of candidateBinaries({
    cwd,
    env,
    wrapperFile: currentWrapperFile,
  })) {
    const status = executableStatus(candidate.path)
    if (status.ok) {
      const realCandidate = realpathSafe(candidate.path)
      if (realCandidate !== undefined && wrapperRealpaths.has(realCandidate)) {
        checked.push(formatChecked(candidate.source, candidate.path, "wrapper shim skipped"))
        continue
      }
      return candidate.path
    }

    checked.push(formatChecked(candidate.source, candidate.path, status.reason))
  }

  throw new TypeweldWrapperError(formatMissingDiagnostic(checked), checked)
}

export function launchTypeweld(
  binary,
  args = process.argv.slice(2),
  env = process.env,
) {
  const child = spawn(binary, [...args], {
    env,
    stdio: "inherit",
    windowsHide: true,
  })

  for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
    process.once(signal, () => {
      child.kill(signal)
    })
  }

  child.once("error", (error) => {
    console.error(
      [
        `failed to start typeweld CLI at \`${binary}\`: ${error.message}`,
        "",
        "Build the CLI with:",
        "  cargo build -p typeweld-cli --bin typeweld",
        "",
        "Or set TYPEWELD_BINARY to the absolute path of a working typeweld binary.",
      ].join("\n"),
    )
    process.exit(1)
  })

  child.once("exit", (code, signal) => {
    if (signal !== null) {
      exitFromSignal(signal)
      return
    }

    process.exit(code ?? 1)
  })

  return child
}

export function main() {
  try {
    launchTypeweld(resolveTypeweldBinary(), process.argv.slice(2))
  } catch (error) {
    if (error instanceof Error) {
      console.error(error.message)
    } else {
      console.error(String(error))
    }
    process.exit(1)
  }
}

if (isMainModule(wrapperFile, process.argv[1])) {
  main()
}

function candidateBinaries(options) {
  const packageRoots = findPackageRoots(dirname(options.wrapperFile))
  const candidates = []

  for (const packageRoot of packageRoots) {
    candidates.push(
      {
        source: "packaged binary",
        path: join(packageRoot, "bin", `${process.platform}-${process.arch}`, platformBinaryName),
      },
      {
        source: "packaged binary",
        path: join(packageRoot, "bin", platformBinaryName),
      },
    )
  }

  for (const targetDir of cargoTargetDirs(options)) {
    candidates.push(
      {
        source: "local cargo build",
        path: join(targetDir, "debug", platformBinaryName),
      },
      {
        source: "local cargo build",
        path: join(targetDir, "release", platformBinaryName),
      },
    )
  }

  for (const pathCandidate of pathBinaries(options.env)) {
    candidates.push({
      source: "PATH",
      path: pathCandidate,
    })
  }

  return dedupeCandidates(candidates)
}

function cargoTargetDirs(options) {
  const roots = []
  const explicitTargetDir = options.env.CARGO_TARGET_DIR?.trim()

  if (explicitTargetDir !== undefined && explicitTargetDir !== "") {
    roots.push(resolve(options.cwd, explicitTargetDir))
  }

  for (const root of workspaceRoots(options.cwd)) {
    roots.push(join(root, "target"))
  }

  for (const packageRoot of findPackageRoots(dirname(options.wrapperFile))) {
    for (const root of workspaceRoots(packageRoot)) {
      roots.push(join(root, "target"))
    }
  }

  return dedupeStrings(roots)
}

function workspaceRoots(start) {
  const roots = []

  for (const directory of parentDirs(resolve(start))) {
    if (
      existsSync(join(directory, "Cargo.toml")) ||
      existsSync(join(directory, "target"))
    ) {
      roots.push(directory)
    }
  }

  return roots
}

function resolveOverride(env, cwd) {
  for (const envVar of binaryOverrideEnvVars) {
    const raw = env[envVar]?.trim()
    if (raw !== undefined && raw !== "") {
      return {
        source: envVar,
        path: resolve(cwd, raw),
      }
    }
  }

  return undefined
}

function pathBinaries(env) {
  const pathValue = readPathEnv(env)
  if (pathValue === undefined || pathValue === "") {
    return []
  }

  return pathValue
    .split(delimiter)
    .filter((entry) => entry !== "")
    .flatMap((entry) =>
      executableNames().map((name) => resolve(entry, name)),
    )
}

function executableNames() {
  return process.platform === "win32"
    ? [`${binaryName}.exe`, binaryName]
    : [binaryName]
}

function readPathEnv(env) {
  const pathKey = Object.keys(env).find((key) => key.toLowerCase() === "path")
  return pathKey === undefined ? undefined : env[pathKey]
}

function findPackageRoots(start) {
  const roots = []
  for (const directory of parentDirs(resolve(start))) {
    if (existsSync(join(directory, "package.json"))) {
      roots.push(directory)
    }
  }

  return roots
}

function parentDirs(start) {
  const directories = []
  let current = resolve(start)

  while (true) {
    directories.push(current)
    const parent = dirname(current)
    if (parent === current) {
      return directories
    }
    current = parent
  }
}

function executableStatus(file) {
  if (!existsSync(file)) {
    return { ok: false, reason: "it does not exist" }
  }

  let stat
  try {
    stat = statSync(file)
  } catch (error) {
    return {
      ok: false,
      reason: `it could not be inspected: ${errorMessage(error)}`,
    }
  }

  if (!stat.isFile()) {
    return { ok: false, reason: "it is not a file" }
  }

  try {
    accessSync(file, process.platform === "win32" ? constants.F_OK : constants.X_OK)
  } catch {
    return { ok: false, reason: "it is not executable" }
  }

  return { ok: true, reason: "found" }
}

function realpathSet(files) {
  const paths = new Set()
  for (const file of files) {
    const realpath = file === undefined ? undefined : realpathSafe(file)
    if (realpath !== undefined) {
      paths.add(realpath)
    }
  }
  return paths
}

function realpathSafe(file) {
  try {
    return realpathSync(file)
  } catch {
    return undefined
  }
}

function dedupeStrings(values) {
  return [...new Set(values)]
}

function dedupeCandidates(candidates) {
  const seen = new Set()
  const deduped = []

  for (const candidate of candidates) {
    const key = `${candidate.source}\0${candidate.path}`
    if (seen.has(key)) {
      continue
    }
    seen.add(key)
    deduped.push(candidate)
  }

  return deduped
}

function formatChecked(source, file, reason) {
  return `${source}: ${file} (${reason})`
}

function formatMissingDiagnostic(checked) {
  const maxLines = 24
  const displayed = checked.slice(0, maxLines)
  const omitted = checked.length - displayed.length
  const checkedLines =
    displayed.length === 0
      ? ["  - no candidate locations were available"]
      : displayed.map((line) => `  - ${line}`)

  if (omitted > 0) {
    checkedLines.push(`  - ... ${omitted} more candidates omitted`)
  }

  return [
    "typeweld npm wrapper could not find the Rust typeweld CLI binary.",
    "",
    "Checked:",
    ...checkedLines,
    "",
    "Build the CLI with:",
    "  cargo build -p typeweld-cli --bin typeweld",
    "",
    "Packaged npm releases should include a binary under bin/<platform>-<arch>/typeweld.",
    "Or set TYPEWELD_BINARY to the absolute path of a working typeweld binary.",
  ].join("\n")
}

function isMainModule(moduleFile, invocationFile) {
  if (invocationFile === undefined) {
    return false
  }

  const moduleRealpath = realpathSafe(moduleFile)
  const invocationRealpath = realpathSafe(invocationFile)

  return (
    moduleRealpath !== undefined &&
    invocationRealpath !== undefined &&
    moduleRealpath === invocationRealpath
  )
}

function exitFromSignal(signal) {
  try {
    process.kill(process.pid, signal)
  } catch {
    process.exit(1)
  }

  setTimeout(() => process.exit(1), 250).unref()
}

function errorMessage(error) {
  return error instanceof Error ? error.message : String(error)
}
