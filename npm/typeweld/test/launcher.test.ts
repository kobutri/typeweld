import { spawnSync, type SpawnSyncReturns } from "node:child_process"
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { delimiter, dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { afterEach, beforeEach, expect, it } from "vitest"
import { resolveTypeweldBinary, TypeweldResolveError } from "../src/index"

const testDir = dirname(fileURLToPath(import.meta.url))
const packageSourceRoot = resolve(testDir, "..")
const launcherSourcePath = join(packageSourceRoot, "src", "index.ts")
const executableName =
  process.platform === "win32" ? "typeweld.exe" : "typeweld"
const platformPackage = `@typeweld/cli-${process.platform}-${process.arch}`

let temp: string

beforeEach(() => {
  temp = mkdtempSync(join(tmpdir(), "typeweld-launcher-"))
})

afterEach(() => {
  rmSync(temp, { recursive: true, force: true })
})

it("prefers TYPEWELD_BINARY over every other candidate", () => {
  const fixture = createFixture({
    cargo: true,
    override: true,
    path: true,
    platformPackage: true,
  })

  const resolved = resolveTypeweldBinary({
    cwd: temp,
    env: fixture.env({ TYPEWELD_BINARY: fixture.overrideBinary }),
    packageRoot: fixture.packageRoot,
  })

  expect(resolved).toBe(fixture.overrideBinary)
})

it("prefers the platform package binary over PATH and cargo builds", () => {
  const fixture = createFixture({
    cargo: true,
    path: true,
    platformPackage: true,
  })

  const resolved = resolveTypeweldBinary({
    cwd: temp,
    env: fixture.env(),
    packageRoot: fixture.packageRoot,
  })

  expect(resolved).toBe(fixture.platformPackageBinary)
})

it("prefers a typeweld binary on PATH over local cargo builds", () => {
  const fixture = createFixture({ cargo: true, path: true })

  const resolved = resolveTypeweldBinary({
    cwd: temp,
    env: fixture.env(),
    packageRoot: fixture.packageRoot,
  })

  expect(resolved).toBe(fixture.pathBinary)
})

it("falls back to the local cargo build relative to the repo root", () => {
  const fixture = createFixture({ cargo: true })

  const resolved = resolveTypeweldBinary({
    cwd: temp,
    env: fixture.env(),
    packageRoot: fixture.packageRoot,
  })

  expect(resolved).toBe(fixture.cargoBinary)
})

it("fails with a diagnostic listing every checked location", () => {
  const fixture = createFixture({})

  let caught: unknown
  try {
    resolveTypeweldBinary({
      cwd: temp,
      env: fixture.env(),
      packageRoot: fixture.packageRoot,
    })
  } catch (error) {
    caught = error
  }

  expect(caught).toBeInstanceOf(TypeweldResolveError)
  const error = caught as TypeweldResolveError
  expect(error.message).toMatch(/could not find the typeweld binary/)
  expect(error.message).toMatch(/TYPEWELD_BINARY: not set/)
  expect(error.message).toMatch(/platform package/)
  expect(error.message).toMatch(/PATH: no/)
  expect(error.message).toMatch(/local cargo build/)
  expect(error.message).toMatch(/cargo build -p typeweld-cli --bin typeweld/)
})

it("rejects a TYPEWELD_BINARY override that does not exist", () => {
  const fixture = createFixture({ cargo: true })
  const missing = join(temp, "missing", executableName)

  expect(() =>
    resolveTypeweldBinary({
      cwd: temp,
      env: fixture.env({ TYPEWELD_BINARY: missing }),
      packageRoot: fixture.packageRoot,
    }),
  ).toThrow(/TYPEWELD_BINARY points to/)
})

it("spawns the binary with forwarded argv, stdio, and exit code", () => {
  const fixture = createFixture({})
  const fakeCli = join(temp, executableName)
  writeExecutable(
    fakeCli,
    [
      "const chunks = []",
      'process.stdin.setEncoding("utf8")',
      'process.stdin.on("data", (chunk) => chunks.push(chunk))',
      'process.stdin.on("end", () => {',
      "  process.stdout.write(JSON.stringify({",
      "    argv: process.argv.slice(2),",
      '    stdin: chunks.join(""),',
      "  }))",
      "  process.exit(7)",
      "})",
      "",
    ].join("\n"),
  )

  const result = runLauncher(
    ["generate", "--manifest", "typeweld.toml"],
    fixture.env({ TYPEWELD_BINARY: fakeCli }),
    "hello from npm",
  )

  expect(result.status).toBe(7)
  expect(result.stderr).toBe("")
  expect(JSON.parse(result.stdout)).toEqual({
    argv: ["generate", "--manifest", "typeweld.toml"],
    stdin: "hello from npm",
  })
})

type FixtureOptions = {
  cargo?: boolean
  override?: boolean
  path?: boolean
  platformPackage?: boolean
}

function createFixture(options: FixtureOptions) {
  const packageRoot = join(temp, "repo", "npm", "typeweld")
  const cargoBinary = join(temp, "repo", "target", "debug", executableName)
  const pathDir = join(temp, "pathbin")
  const pathBinary = join(pathDir, executableName)
  const overrideBinary = join(temp, "override", executableName)
  const platformPackageRoot = join(
    packageRoot,
    "node_modules",
    platformPackage,
  )
  const platformPackageBinary = join(
    platformPackageRoot,
    "bin",
    `${process.platform}-${process.arch}`,
    executableName,
  )

  mkdirSync(packageRoot, { recursive: true })
  writeFileSync(join(packageRoot, "package.json"), '{"name":"typeweld"}\n')
  mkdirSync(pathDir, { recursive: true })

  if (options.platformPackage === true) {
    mkdirSync(dirname(platformPackageBinary), { recursive: true })
    writeFileSync(
      join(platformPackageRoot, "package.json"),
      `{"name":"${platformPackage}"}\n`,
    )
    writeExecutable(platformPackageBinary, "process.exit(0)\n")
  }
  if (options.path === true) {
    writeExecutable(pathBinary, "process.exit(0)\n")
  }
  if (options.cargo === true) {
    mkdirSync(dirname(cargoBinary), { recursive: true })
    writeExecutable(cargoBinary, "process.exit(0)\n")
  }
  if (options.override === true) {
    mkdirSync(dirname(overrideBinary), { recursive: true })
    writeExecutable(overrideBinary, "process.exit(0)\n")
  }

  return {
    cargoBinary,
    overrideBinary,
    packageRoot,
    pathBinary,
    platformPackageBinary,
    env(overrides: NodeJS.ProcessEnv = {}): NodeJS.ProcessEnv {
      return {
        ...cleanEnv(),
        PATH: pathDir,
        ...overrides,
      }
    },
  }
}

function runLauncher(
  args: readonly string[],
  env: NodeJS.ProcessEnv,
  input = "",
): SpawnSyncReturns<string> {
  return spawnSync(
    process.execPath,
    ["--import", "tsx", launcherSourcePath, ...args],
    {
      cwd: packageSourceRoot,
      encoding: "utf8",
      env: {
        ...env,
        PATH: [dirname(process.execPath), env.PATH ?? ""]
          .filter((entry) => entry !== "")
          .join(delimiter),
      },
      input,
    },
  )
}

function cleanEnv(): NodeJS.ProcessEnv {
  const env = { ...process.env }
  delete env.TYPEWELD_BINARY
  delete env.PATH
  return env
}

function writeExecutable(path: string, body: string): void {
  writeFileSync(path, `#!/usr/bin/env node\n${body}`)
  chmodSync(path, 0o755)
}
