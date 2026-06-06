#!/usr/bin/env node
// @ts-check

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

export const binaryName = "api-ls"
export const binaryOverrideEnvVars = ["API_LS_BINARY", "RUST_TS_API_LS"]

const wrapperFile = fileURLToPath(import.meta.url)
const platformBinaryName = process.platform === "win32" ? `${binaryName}.exe` : binaryName

export class ApiLsWrapperError extends Error {
  /** @type {ReadonlyArray<string>} */
  checked

  /**
   * @param {string} message
   * @param {ReadonlyArray<string>} checked
   */
  constructor(message, checked) {
    super(message)
    this.name = "ApiLsWrapperError"
    this.checked = checked
  }
}

/**
 * @typedef ResolveOptions
 * @property {NodeJS.ProcessEnv=} env
 * @property {string=} cwd
 * @property {string=} wrapperFile
 * @property {string=} invocationFile
 */

/**
 * Resolves the Rust gateway binary used by the npm launcher.
 *
 * Resolution order intentionally favors explicit and package-local binaries
 * before workspace build outputs or PATH, so a published package can carry its
 * own gateway without being shadowed by an unrelated development binary.
 *
 * @param {ResolveOptions=} options
 * @returns {string}
 */
export function resolveApiLsBinary(options = {}) {
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

    throw new ApiLsWrapperError(
      [
        `${override.source} points to \`${override.path}\`, but ${status.reason}.`,
        "",
        "Build the gateway with:",
        "  cargo build -p api-ls --bin api-ls",
        "",
        "Or set API_LS_BINARY to the absolute path of a working api-ls binary.",
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

  throw new ApiLsWrapperError(formatMissingDiagnostic(checked), checked)
}

/**
 * Starts the Rust gateway and keeps every stdio stream inherited from the
 * wrapper process. Editors can therefore treat this launcher exactly like the
 * real language-server binary.
 *
 * @param {string} binary
 * @param {ReadonlyArray<string>} args
 * @returns {import("node:child_process").ChildProcess}
 */
export function launchApiLs(binary, args = process.argv.slice(2)) {
  const child = spawn(binary, [...args], {
    stdio: "inherit",
    windowsHide: true,
  })

  /** @type {ReadonlyArray<NodeJS.Signals>} */
  const forwardedSignals = ["SIGINT", "SIGTERM", "SIGHUP"]

  for (const signal of forwardedSignals) {
    process.once(signal, () => {
      child.kill(signal)
    })
  }

  child.once("error", (error) => {
    console.error(
      [
        `failed to start api-ls gateway at \`${binary}\`: ${error.message}`,
        "",
        "Build the gateway with:",
        "  cargo build -p api-ls --bin api-ls",
        "",
        "Or set API_LS_BINARY to the absolute path of a working api-ls binary.",
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
    launchApiLs(resolveApiLsBinary(), process.argv.slice(2))
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

/**
 * @param {{ cwd: string, env: NodeJS.ProcessEnv, wrapperFile: string }} options
 * @returns {ReadonlyArray<{ source: string, path: string }>}
 */
function candidateBinaries(options) {
  const packageRoot = findPackageRoot(dirname(options.wrapperFile))
  const candidates = []

  if (packageRoot !== undefined) {
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

/**
 * @param {{ cwd: string, env: NodeJS.ProcessEnv, wrapperFile: string }} options
 * @returns {ReadonlyArray<string>}
 */
function cargoTargetDirs(options) {
  const roots = []
  const explicitTargetDir = options.env.CARGO_TARGET_DIR?.trim()

  if (explicitTargetDir !== undefined && explicitTargetDir !== "") {
    roots.push(resolve(options.cwd, explicitTargetDir))
  }

  for (const root of workspaceRoots(options.cwd)) {
    roots.push(join(root, "target"))
  }

  const packageRoot = findPackageRoot(dirname(options.wrapperFile))
  if (packageRoot !== undefined) {
    for (const root of workspaceRoots(packageRoot)) {
      roots.push(join(root, "target"))
    }
  }

  return dedupeStrings(roots)
}

/**
 * @param {string} start
 * @returns {ReadonlyArray<string>}
 */
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

/**
 * @param {NodeJS.ProcessEnv} env
 * @param {string} cwd
 * @returns {{ source: string, path: string } | undefined}
 */
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

/**
 * @param {NodeJS.ProcessEnv} env
 * @returns {ReadonlyArray<string>}
 */
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

/**
 * @returns {ReadonlyArray<string>}
 */
function executableNames() {
  return process.platform === "win32"
    ? [`${binaryName}.exe`, binaryName]
    : [binaryName]
}

/**
 * @param {NodeJS.ProcessEnv} env
 * @returns {string | undefined}
 */
function readPathEnv(env) {
  const pathKey = Object.keys(env).find((key) => key.toLowerCase() === "path")
  return pathKey === undefined ? undefined : env[pathKey]
}

/**
 * @param {string} start
 * @returns {string | undefined}
 */
function findPackageRoot(start) {
  for (const directory of parentDirs(resolve(start))) {
    if (existsSync(join(directory, "package.json"))) {
      return directory
    }
  }

  return undefined
}

/**
 * @param {string} start
 * @returns {ReadonlyArray<string>}
 */
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

/**
 * @param {string} file
 * @returns {{ ok: true, reason: "found" } | { ok: false, reason: string }}
 */
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

/**
 * @param {ReadonlyArray<string | undefined>} files
 * @returns {Set<string>}
 */
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

/**
 * @param {string} file
 * @returns {string | undefined}
 */
function realpathSafe(file) {
  try {
    return realpathSync(file)
  } catch {
    return undefined
  }
}

/**
 * @param {ReadonlyArray<string>} values
 * @returns {ReadonlyArray<string>}
 */
function dedupeStrings(values) {
  return [...new Set(values)]
}

/**
 * @param {ReadonlyArray<{ source: string, path: string }>} candidates
 * @returns {ReadonlyArray<{ source: string, path: string }>}
 */
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

/**
 * @param {string} source
 * @param {string} file
 * @param {string} reason
 * @returns {string}
 */
function formatChecked(source, file, reason) {
  return `${source}: ${file} (${reason})`
}

/**
 * @param {ReadonlyArray<string>} checked
 * @returns {string}
 */
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
    "api-ls npm wrapper could not find the Rust api-ls gateway binary.",
    "",
    "Checked:",
    ...checkedLines,
    "",
    "Build the gateway with:",
    "  cargo build -p api-ls --bin api-ls",
    "",
    "Or set API_LS_BINARY to the absolute path of a working api-ls binary.",
  ].join("\n")
}

/**
 * @param {string} moduleFile
 * @param {string | undefined} invocationFile
 * @returns {boolean}
 */
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

/**
 * @param {NodeJS.Signals} signal
 */
function exitFromSignal(signal) {
  try {
    process.kill(process.pid, signal)
  } catch {
    process.exit(1)
  }

  setTimeout(() => process.exit(1), 250).unref()
}

/**
 * @param {unknown} error
 * @returns {string}
 */
function errorMessage(error) {
  return error instanceof Error ? error.message : String(error)
}
