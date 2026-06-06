import { spawn, execFileSync } from "node:child_process"
import {
  mkdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs"
import { dirname, join, relative, resolve, sep } from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"

const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const repoRoot = resolve(appRoot, "../../..")
const outRoot = join(repoRoot, "target/examples/e2e-axum-effect/runtime-js")
const generatedPackageDir = join(
  repoRoot,
  "target/api-contract/effect-v4/packages/_workspace_e2e-api",
)
const tsc = join(repoRoot, "npm/node_modules/typescript/bin/tsc")
const serverBinary = join(
  repoRoot,
  "target/debug",
  process.platform === "win32"
    ? "e2e-axum-effect-server.exe"
    : "e2e-axum-effect-server",
)

rmSync(outRoot, { recursive: true, force: true })
mkdirSync(outRoot, { recursive: true })
mkdirSync(join(outRoot, "node_modules/@rust-ts-integration"), { recursive: true })
mkdirSync(join(outRoot, "node_modules/@workspace"), { recursive: true })

compileEffectRuntime()
linkEffectPackage()
compileGeneratedPackage()
writeGeneratedPackageManifest()
compileApp()
buildServer()

const server = spawn(
  serverBinary,
  ["serve", "127.0.0.1:0"],
  {
    cwd: repoRoot,
    stdio: ["ignore", "pipe", "inherit"],
  },
)

try {
  const baseUrl = await waitForListening(server)
  const runtimeModule = await import(pathToFileURL(join(outRoot, "app/runtime.js")).href)
  await runtimeModule.runAgainstServer(baseUrl)
  console.log("e2e Axum + Effect runtime checks passed")
} finally {
  server.kill("SIGTERM")
}

function compileEffectRuntime() {
  const runtimeOut = join(
    outRoot,
    "node_modules/@rust-ts-integration/effect-runtime",
  )
  runTsc([
    "--project",
    join(repoRoot, "npm/effect-runtime/tsconfig.json"),
    "--outDir",
    runtimeOut,
    "--tsBuildInfoFile",
    join(outRoot, "effect-runtime.tsbuildinfo"),
    "--noEmit",
    "false",
  ])

  writeJson(join(runtimeOut, "package.json"), {
    type: "module",
    types: "./index.d.ts",
    exports: {
      ".": {
        types: "./index.d.ts",
        import: "./index.js",
      },
      "./compat": {
        types: "./compat.d.ts",
        import: "./compat.js",
      },
      "./*": "./*",
    },
  })
}

function linkEffectPackage() {
  const linkPath = join(outRoot, "node_modules/effect")
  rmSync(linkPath, { recursive: true, force: true })
  symlinkSync(
    join(repoRoot, "npm/node_modules/effect"),
    linkPath,
    process.platform === "win32" ? "junction" : "dir",
  )
}

function compileGeneratedPackage() {
  const packageOut = join(outRoot, "node_modules/@workspace/e2e-api")
  const rootDir = relativeTsPath(outRoot, generatedPackageDir)
  const tsconfig = join(outRoot, "generated-tsconfig.json")
  writeJson(tsconfig, {
    compilerOptions: baseCompilerOptions({
      declaration: true,
      outDir: "node_modules/@workspace/e2e-api",
      paths: runtimePackagePaths(),
      rootDir,
      tsBuildInfoFile: "e2e-api.tsbuildinfo",
    }),
    include: [`${rootDir}/*.ts`],
  })
  runTsc(["--project", tsconfig], outRoot)
  mkdirSync(packageOut, { recursive: true })
}

function writeGeneratedPackageManifest() {
  writeJson(join(outRoot, "node_modules/@workspace/e2e-api/package.json"), {
    type: "module",
    types: "./index.d.ts",
    exports: {
      ".": {
        types: "./index.d.ts",
        import: "./index.js",
      },
      "./schemas": {
        types: "./schemas.d.ts",
        import: "./schemas.js",
      },
      "./errors": {
        types: "./errors.d.ts",
        import: "./errors.js",
      },
      "./endpoints": {
        types: "./endpoints.d.ts",
        import: "./endpoints.js",
      },
      "./layer": {
        types: "./layer.d.ts",
        import: "./layer.js",
      },
    },
  })
}

function compileApp() {
  const appSrc = join(appRoot, "src")
  const rootDir = relativeTsPath(outRoot, appSrc)
  const tsconfig = join(outRoot, "app-tsconfig.json")
  writeJson(tsconfig, {
    compilerOptions: baseCompilerOptions({
      outDir: "app",
      paths: {
        ...runtimePackagePaths(),
        "@workspace/e2e-api": ["node_modules/@workspace/e2e-api/index.d.ts"],
        "@workspace/e2e-api/*": ["node_modules/@workspace/e2e-api/*"],
      },
      rootDir,
      tsBuildInfoFile: "app.tsbuildinfo",
    }),
    include: [`${rootDir}/**/*.ts`],
  })
  runTsc(["--project", tsconfig], outRoot)
}

function buildServer() {
  execFileSync("cargo", ["build", "-q", "-p", "e2e-axum-effect-server"], {
    cwd: repoRoot,
    stdio: "inherit",
  })
}

function baseCompilerOptions(extra) {
  return {
    baseUrl: ".",
    exactOptionalPropertyTypes: true,
    lib: ["DOM", "ES2022", "ESNext.Disposable"],
    module: "NodeNext",
    moduleResolution: "NodeNext",
    noUncheckedIndexedAccess: true,
    skipLibCheck: true,
    strict: true,
    target: "ES2022",
    ...extra,
  }
}

function runtimePackagePaths() {
  return {
    "@rust-ts-integration/effect-runtime": [
      "node_modules/@rust-ts-integration/effect-runtime/index.d.ts",
    ],
    "@rust-ts-integration/effect-runtime/compat": [
      "node_modules/@rust-ts-integration/effect-runtime/compat.d.ts",
    ],
    "@rust-ts-integration/effect-runtime/*": [
      "node_modules/@rust-ts-integration/effect-runtime/*",
    ],
    effect: ["node_modules/effect/dist/index.d.ts"],
  }
}

function runTsc(args, cwd = repoRoot) {
  execFileSync(process.execPath, [tsc, ...args], {
    cwd,
    stdio: "inherit",
  })
}

function waitForListening(child) {
  return new Promise((resolveListening, reject) => {
    let output = ""
    const timeout = setTimeout(() => {
      reject(new Error(`server did not print LISTENING line; stdout so far:\n${output}`))
    }, 30_000)

    child.stdout.setEncoding("utf8")
    child.stdout.on("data", (chunk) => {
      output += chunk
      const line = output.split(/\r?\n/).find((candidate) =>
        candidate.startsWith("LISTENING "),
      )
      if (line !== undefined) {
        clearTimeout(timeout)
        resolveListening(line.slice("LISTENING ".length).trim())
      }
    })
    child.once("exit", (code, signal) => {
      clearTimeout(timeout)
      reject(new Error(`server exited before listening; code=${code} signal=${signal}`))
    })
    child.once("error", (error) => {
      clearTimeout(timeout)
      reject(error)
    })
  })
}

function relativeTsPath(from, to) {
  const path = relative(from, to)
  return path === "" ? "." : path.split(sep).join("/")
}

function writeJson(path, value) {
  mkdirSync(dirname(path), { recursive: true })
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`)
}
