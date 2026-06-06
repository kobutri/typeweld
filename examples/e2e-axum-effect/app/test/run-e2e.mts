import { spawn, execFileSync } from "node:child_process"
import type { ChildProcess } from "node:child_process"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const repoRoot = resolve(appRoot, "../../..")
const serverBinary = join(
  repoRoot,
  "target/debug",
  process.platform === "win32"
    ? "e2e-axum-effect-server.exe"
    : "e2e-axum-effect-server",
)

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
  const runtimeModule = await import("../src/runtime")
  await runtimeModule.runAgainstServer(baseUrl)
  console.log("e2e Axum + Effect runtime checks passed")
} finally {
  server.kill("SIGTERM")
}

function buildServer(): void {
  execFileSync("cargo", ["build", "-q", "-p", "e2e-axum-effect-server"], {
    cwd: repoRoot,
    stdio: "inherit",
  })
}

function waitForListening(child: ChildProcess): Promise<string> {
  return new Promise((resolveListening, reject) => {
    let output = ""
    const timeout = setTimeout(() => {
      reject(new Error(`server did not print LISTENING line; stdout so far:\n${output}`))
    }, 30_000)

    if (child.stdout === null) {
      clearTimeout(timeout)
      reject(new Error("server stdout was not piped"))
      return
    }

    child.stdout.setEncoding("utf8")
    child.stdout.on("data", (chunk: string) => {
      output += chunk
      const line = output.split(/\r?\n/).find((candidate) =>
        candidate.startsWith("LISTENING "),
      )
      if (line !== undefined) {
        clearTimeout(timeout)
        resolveListening(line.slice("LISTENING ".length).trim())
      }
    })
    child.once("exit", (code: number | null, signal: NodeJS.Signals | null) => {
      clearTimeout(timeout)
      reject(new Error(`server exited before listening; code=${code} signal=${signal}`))
    })
    child.once("error", (error: Error) => {
      clearTimeout(timeout)
      reject(error)
    })
  })
}
