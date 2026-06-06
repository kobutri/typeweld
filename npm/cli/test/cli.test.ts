import { spawnSync } from "node:child_process"
import {
  chmodSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, delimiter, join, resolve } from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"
import { expect, it } from "@effect/vitest"
import type { SpawnSyncReturns } from "node:child_process"

const testDir = dirname(fileURLToPath(import.meta.url))
const packageRoot = resolve(testDir, "..")
const wrapperPath = join(packageRoot, "dist", "index.js")
const wrapperModule = await import(pathToFileURL(wrapperPath).href)
const binaryName = process.platform === "win32" ? "typeweld.exe" : "typeweld"

it("package exposes typeweld as an executable bin", () => {
  const packageJson = JSON.parse(
    readFileSync(join(packageRoot, "package.json"), "utf8"),
  )

  expect(packageJson.bin.typeweld).toBe("./dist/index.js")
})

it("resolves a packaged CLI binary before workspace and PATH candidates", () => {
  const temp = createTempDir()
  try {
    const fakePackageRoot = join(temp, "typeweld")
    const fakeWrapper = join(fakePackageRoot, "dist", "index.js")
    const fakeCli = join(fakePackageRoot, "bin", binaryName)
    mkdirSync(dirname(fakeWrapper), { recursive: true })
    mkdirSync(dirname(fakeCli), { recursive: true })
    writeFileSync(join(fakePackageRoot, "package.json"), "{}")
    writeExecutable(fakeCli, "process.exit(0)\n")

    const resolved = wrapperModule.resolveTypeweldBinary({
      cwd: temp,
      env: cleanEnv(),
      invocationFile: fakeWrapper,
      wrapperFile: fakeWrapper,
    })

    expect(resolved).toBe(fakeCli)
  } finally {
    rmSync(temp, { recursive: true, force: true })
  }
})

it("resolves a local cargo build CLI from the current workspace", () => {
  const temp = createTempDir()
  try {
    const workspace = join(temp, "workspace")
    const fakeWrapper = join(temp, "package", "dist", "index.js")
    const fakeCli = join(workspace, "target", "debug", binaryName)
    mkdirSync(dirname(fakeWrapper), { recursive: true })
    mkdirSync(dirname(fakeCli), { recursive: true })
    writeFileSync(join(workspace, "Cargo.toml"), "[workspace]\n")
    writeExecutable(fakeCli, "process.exit(0)\n")

    const resolved = wrapperModule.resolveTypeweldBinary({
      cwd: workspace,
      env: cleanEnv(),
      invocationFile: fakeWrapper,
      wrapperFile: fakeWrapper,
    })

    expect(resolved).toBe(fakeCli)
  } finally {
    rmSync(temp, { recursive: true, force: true })
  }
})

it("forwards arguments and stdio to the CLI", () => {
  const temp = createTempDir()
  try {
    const fakeCli = join(temp, binaryName)
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
        "})",
        "",
      ].join("\n"),
    )

    const result = runWrapper(["new", "demo", "--yes"], {
      ...cleanEnv(),
      TYPEWELD_BINARY: fakeCli,
    }, "hello from npm")

    expect(result.status).toBe(0)
    expect(result.stderr).toBe("")
    expect(JSON.parse(result.stdout)).toEqual({
      argv: ["new", "demo", "--yes"],
      stdin: "hello from npm",
    })
  } finally {
    rmSync(temp, { recursive: true, force: true })
  }
})

it("prints clear diagnostics when the configured CLI is missing", () => {
  const temp = createTempDir()
  try {
    const missingCli = join(temp, binaryName)
    const result = runWrapper([], {
      ...cleanEnv(),
      TYPEWELD_BINARY: missingCli,
    })

    expect(result.status).toBe(1)
    expect(result.stdout).toBe("")
    expect(result.stderr).toMatch(/TYPEWELD_BINARY points to/)
    expect(result.stderr).toMatch(/it does not exist/)
    expect(result.stderr).toMatch(/cargo build -p typeweld-cli --bin typeweld/)
    expect(result.stderr).toMatch(/set TYPEWELD_BINARY/)
  } finally {
    rmSync(temp, { recursive: true, force: true })
  }
})

function runWrapper(
  args: readonly string[],
  env: NodeJS.ProcessEnv,
  input = "",
): SpawnSyncReturns<string> {
  return spawnSync(process.execPath, [wrapperPath, ...args], {
    cwd: packageRoot,
    encoding: "utf8",
    env,
    input,
  })
}

function cleanEnv(): NodeJS.ProcessEnv {
  const env = { ...process.env }
  delete env.API_CLI_BINARY
  delete env.TYPEWELD_BINARY
  delete env.CARGO_TARGET_DIR

  env.PATH = [dirname(process.execPath), process.env.PATH ?? ""]
    .filter((entry) => entry !== "")
    .join(delimiter)

  return env
}

function createTempDir(): string {
  return mkdtempSync(join(tmpdir(), "typeweld-cli-wrapper-"))
}

function writeExecutable(path: string, body: string): void {
  writeFileSync(path, `#!/usr/bin/env node\n${body}`)
  chmodSync(path, 0o755)
}
