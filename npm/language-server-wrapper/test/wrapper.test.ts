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

const testDir = dirname(fileURLToPath(import.meta.url))
const packageRoot = resolve(testDir, "..")
const wrapperPath = join(packageRoot, "src", "index.ts")
const tsxCli = join(packageRoot, "..", "node_modules", "tsx", "dist", "cli.mjs")
const wrapperModule = await import(pathToFileURL(wrapperPath).href)
const binaryName = process.platform === "win32" ? "api-ls.exe" : "api-ls"

it("package exposes api-ls as an executable bin", () => {
  const packageJson = JSON.parse(
    readFileSync(join(packageRoot, "package.json"), "utf8"),
  )

  expect(packageJson.bin["api-ls"]).toBe("./src/index.ts")
})

it("resolves a packaged gateway binary before workspace and PATH candidates", () => {
  const temp = createTempDir()
  try {
    const fakePackageRoot = join(temp, "language-server")
    const fakeWrapper = join(fakePackageRoot, "src", "index.ts")
    const fakeGateway = join(fakePackageRoot, "bin", binaryName)
    mkdirSync(dirname(fakeWrapper), { recursive: true })
    mkdirSync(dirname(fakeGateway), { recursive: true })
    writeFileSync(join(fakePackageRoot, "package.json"), "{}")
    writeExecutable(fakeGateway, "process.exit(0)\n")

    const resolved = wrapperModule.resolveApiLsBinary({
      cwd: temp,
      env: cleanEnv(),
      invocationFile: fakeWrapper,
      wrapperFile: fakeWrapper,
    })

    expect(resolved).toBe(fakeGateway)
  } finally {
    rmSync(temp, { recursive: true, force: true })
  }
})

it("resolves a local cargo build gateway from the current workspace", () => {
  const temp = createTempDir()
  try {
    const workspace = join(temp, "workspace")
    const fakeWrapper = join(temp, "package", "src", "index.ts")
    const fakeGateway = join(workspace, "target", "debug", binaryName)
    mkdirSync(dirname(fakeWrapper), { recursive: true })
    mkdirSync(dirname(fakeGateway), { recursive: true })
    writeFileSync(join(workspace, "Cargo.toml"), "[workspace]\n")
    writeExecutable(fakeGateway, "process.exit(0)\n")

    const resolved = wrapperModule.resolveApiLsBinary({
      cwd: workspace,
      env: cleanEnv(),
      invocationFile: fakeWrapper,
      wrapperFile: fakeWrapper,
    })

    expect(resolved).toBe(fakeGateway)
  } finally {
    rmSync(temp, { recursive: true, force: true })
  }
})

it("forwards arguments and stdio to the gateway", () => {
  const temp = createTempDir()
  try {
    const fakeGateway = join(temp, binaryName)
    writeExecutable(
      fakeGateway,
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

    const result = runWrapper(["--stdio", "--trace", "verbose"], {
      ...cleanEnv(),
      API_LS_BINARY: fakeGateway,
    }, "hello from editor")

    expect(result.status).toBe(0)
    expect(result.stderr).toBe("")
    expect(JSON.parse(result.stdout)).toEqual({
      argv: ["--stdio", "--trace", "verbose"],
      stdin: "hello from editor",
    })
  } finally {
    rmSync(temp, { recursive: true, force: true })
  }
})

it("prints clear diagnostics when the configured gateway is missing", () => {
  const temp = createTempDir()
  try {
    const missingGateway = join(temp, binaryName)
    const result = runWrapper([], {
      ...cleanEnv(),
      API_LS_BINARY: missingGateway,
    })

    expect(result.status).toBe(1)
    expect(result.stdout).toBe("")
    expect(result.stderr).toMatch(/API_LS_BINARY points to/)
    expect(result.stderr).toMatch(/it does not exist/)
    expect(result.stderr).toMatch(/cargo build -p api-ls --bin api-ls/)
    expect(result.stderr).toMatch(/set API_LS_BINARY/)
  } finally {
    rmSync(temp, { recursive: true, force: true })
  }
})

/**
 * @param {ReadonlyArray<string>} args
 * @param {NodeJS.ProcessEnv} env
 * @param {string=} input
 */
function runWrapper(args, env, input = "") {
  return spawnSync(process.execPath, [tsxCli, wrapperPath, ...args], {
    cwd: packageRoot,
    encoding: "utf8",
    env,
    input,
  })
}

/**
 * @returns {NodeJS.ProcessEnv}
 */
function cleanEnv() {
  const env = { ...process.env }
  delete env.API_LS_BINARY
  delete env.RUST_TS_API_LS
  delete env.CARGO_TARGET_DIR

  env.PATH = [dirname(process.execPath), process.env.PATH ?? ""]
    .filter((entry) => entry !== "")
    .join(delimiter)

  return env
}

function createTempDir() {
  return mkdtempSync(join(tmpdir(), "api-ls-wrapper-"))
}

/**
 * @param {string} path
 * @param {string} body
 */
function writeExecutable(path, body) {
  writeFileSync(path, `#!/usr/bin/env node\n${body}`)
  chmodSync(path, 0o755)
}
