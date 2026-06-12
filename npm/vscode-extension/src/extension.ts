// The typeweld VS Code extension.
//
// Typeweld's language server is designed to be the one Rust language server
// the editor runs, transparently wrapping the user's real rust-analyzer. In
// VS Code the rust-analyzer extension owns the Rust client (and all its UX:
// runnables, debugging, the status bar), so the integration routes it
// through typeweld with a one-time, consent-gated rewrite of
// `rust-analyzer.server.path` in workspace settings; the original server is
// preserved and still used behind the proxy. Without the rust-analyzer
// extension, this extension hosts its own client for Rust files.
//
// The TypeScript side needs no client at all: the bundled
// `@typeweld/typescript-plugin` (contributed via `typescriptServerPlugins`)
// runs inside VS Code's own tsserver and connects to the language server
// directly.

import * as fs from "node:fs"
import * as net from "node:net"
import * as path from "node:path"

import * as vscode from "vscode"
import {
  LanguageClient,
  RevealOutputChannelOn,
  type LanguageClientOptions,
  type ServerOptions,
} from "vscode-languageclient/node"

const workspaceMarker = "typeweld.toml"
const executableName =
  process.platform === "win32" ? "typeweld.exe" : "typeweld"
const rustAnalyzerExtensionId = "rust-lang.rust-analyzer"
const declinedKey = "typeweld.integrationDeclined"
const previousServerPathKey = "typeweld.previousRustAnalyzerPath"
const documentSelector = [{ scheme: "file", language: "rust" }]

let extensionContext: vscode.ExtensionContext | undefined
let outputChannel: vscode.LogOutputChannel | undefined
let client: LanguageClient | undefined
let daemonLink: DaemonLink | undefined
let reconcileQueue = Promise.resolve()

/**
 * A connection to the typeweld language server's local socket (discovered
 * through `target/typeweld/ls.json`), as an "editor" session. The server
 * asks us to save the files its cross-language edits touched: VS Code
 * auto-saves files affected by its own rename refactorings, but a
 * server-applied `workspace/applyEdit` gets no such treatment.
 */
class DaemonLink {
  private socket: net.Socket | undefined
  private buffer = ""
  private readonly timer: NodeJS.Timeout

  constructor(private readonly root: string) {
    this.connect()
    this.timer = setInterval(() => {
      if (this.socket === undefined) {
        this.connect()
      }
    }, 5000)
  }

  dispose(): void {
    clearInterval(this.timer)
    this.socket?.destroy()
    this.socket = undefined
  }

  private connect(): void {
    let discovered: { port?: number; token?: string }
    try {
      discovered = JSON.parse(
        fs.readFileSync(
          path.join(this.root, "target", "typeweld", "ls.json"),
          "utf8",
        ),
      ) as { port?: number; token?: string }
    } catch {
      return
    }
    if (typeof discovered.port !== "number" || typeof discovered.token !== "string") {
      return
    }
    const socket = net.connect(discovered.port, "127.0.0.1")
    socket.setNoDelay(true)
    socket.on("connect", () => {
      this.socket = socket
      this.buffer = ""
      socket.write(
        JSON.stringify({
          method: "hello",
          kind: "editor",
          token: discovered.token,
          pid: process.pid,
        }) + "\n",
      )
      log("Connected to the typeweld language server socket.")
    })
    socket.on("data", (data) => this.onData(data.toString()))
    const drop = () => {
      if (this.socket === socket) this.socket = undefined
      socket.destroy()
    }
    socket.on("error", drop)
    socket.on("close", drop)
  }

  private onData(chunk: string): void {
    this.buffer += chunk
    let index: number
    while ((index = this.buffer.indexOf("\n")) >= 0) {
      const line = this.buffer.slice(0, index)
      this.buffer = this.buffer.slice(index + 1)
      if (line.trim() === "") continue
      try {
        this.onMessage(JSON.parse(line) as Record<string, unknown>)
      } catch {
        // Malformed message: ignore.
      }
    }
  }

  private onMessage(message: Record<string, unknown>): void {
    if (message["method"] !== "saveFiles" || !Array.isArray(message["files"])) {
      return
    }
    for (const file of message["files"]) {
      if (typeof file !== "string") continue
      void vscode.workspace.save(vscode.Uri.file(file)).then(undefined, () => {
        // The document may already be closed or saved; nothing to do.
      })
    }
  }
}

export function activate(context: vscode.ExtensionContext): void {
  extensionContext = context
  outputChannel = vscode.window.createOutputChannel("Typeweld", { log: true })

  context.subscriptions.push(
    outputChannel,
    vscode.commands.registerCommand("typeweld.server.restart", () => {
      queueReconcile({ restart: true })
    }),
    vscode.commands.registerCommand("typeweld.integration.enable", () => {
      void context.workspaceState.update(declinedKey, undefined).then(() => {
        queueReconcile()
      })
    }),
    vscode.commands.registerCommand("typeweld.integration.disable", () => {
      void disableIntegration()
    }),
    vscode.workspace.onDidChangeWorkspaceFolders(() => {
      queueReconcile()
    }),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("typeweld")) {
        queueReconcile({ restart: true })
      }
    }),
  )

  queueReconcile()
}

export async function deactivate(): Promise<void> {
  extensionContext = undefined
  daemonLink?.dispose()
  daemonLink = undefined
  await stopClient()
}

function queueReconcile(options: { restart?: boolean } = {}): void {
  reconcileQueue = reconcileQueue
    .then(async () => {
      if (options.restart === true) {
        await stopClient()
      }
      await reconcile()
    })
    .catch((error: unknown) => {
      log(`typeweld client reconciliation failed: ${formatError(error)}`)
    })
}

async function reconcile(): Promise<void> {
  if (extensionContext === undefined) {
    return
  }

  const wanted = await hasTypeweldWorkspace()
  if (!wanted) {
    if (client !== undefined) {
      log(
        `No ${workspaceMarker} found in the workspace; stopping the language server.`,
      )
      await stopClient()
    }
    daemonLink?.dispose()
    daemonLink = undefined
    return
  }
  const root = typeweldRoot()
  if (daemonLink === undefined && root !== undefined) {
    daemonLink = new DaemonLink(root)
  }

  // With the rust-analyzer extension installed, that extension stays the
  // Rust client; typeweld slots in behind it as the server.
  if (vscode.extensions.getExtension(rustAnalyzerExtensionId) !== undefined) {
    if (client !== undefined) {
      await stopClient()
    }
    await ensureRustAnalyzerIntegration(extensionContext)
    return
  }

  if (client === undefined) {
    await startClient(extensionContext)
  }
}

/**
 * Routes the rust-analyzer extension through typeweld: one consent prompt,
 * then `rust-analyzer.server.path` (workspace settings) points at the
 * typeweld binary while the original server keeps running behind the proxy
 * via `rust-analyzer.server.extraEnv`.
 */
async function ensureRustAnalyzerIntegration(
  context: vscode.ExtensionContext,
): Promise<void> {
  const binary = resolveServerBinary(context, vscode.workspace.getConfiguration("typeweld"))
  if (binary === undefined) {
    log("Typeweld binary not found; rust-analyzer integration unavailable.")
    return
  }

  const raConfig = vscode.workspace.getConfiguration("rust-analyzer")
  const currentPath = raConfig.get<string>("server.path", "").trim()
  if (currentPath === binary) {
    return
  }
  if (context.workspaceState.get<boolean>(declinedKey) === true) {
    return
  }

  const choice = await vscode.window.showInformationMessage(
    "Typeweld can route rust-analyzer through its language server to enable " +
      "cross-language navigation and renames. This updates " +
      "`rust-analyzer.server.path` in workspace settings; your current " +
      "rust-analyzer keeps running behind it. Enable?",
    "Enable",
    "Not now",
  )
  if (choice !== "Enable") {
    await context.workspaceState.update(declinedKey, true)
    return
  }

  // Preserve the user's server under our own key (we are about to overwrite
  // the only place it lives), and keep using exactly that binary behind the
  // proxy — or the extension-bundled one when nothing was configured.
  await context.workspaceState.update(previousServerPathKey, currentPath)
  const wrapped = currentPath !== "" ? currentPath : bundledRustAnalyzer()
  const extraEnv = {
    ...raConfig.get<Record<string, unknown>>("server.extraEnv", {}),
    TYPEWELD_RUN_LSP: "1",
    ...(wrapped !== undefined ? { TYPEWELD_RUST_ANALYZER: wrapped } : {}),
  }
  await raConfig.update(
    "server.extraEnv",
    extraEnv,
    vscode.ConfigurationTarget.Workspace,
  )
  await raConfig.update(
    "server.path",
    binary,
    vscode.ConfigurationTarget.Workspace,
  )
  log(
    `rust-analyzer now routes through ${binary}` +
      (wrapped !== undefined ? ` (wrapping ${wrapped})` : ""),
  )
  void vscode.commands
    .executeCommand("rust-analyzer.restartServer")
    .then(undefined, () => {
      // Older rust-analyzer builds restart on the config change by themselves.
    })
}

/** Restores the rust-analyzer settings the integration rewrote. */
async function disableIntegration(): Promise<void> {
  const context = extensionContext
  if (context === undefined) {
    return
  }
  const raConfig = vscode.workspace.getConfiguration("rust-analyzer")
  const previous =
    context.workspaceState.get<string>(previousServerPathKey, "") ?? ""
  await raConfig.update(
    "server.path",
    previous === "" ? undefined : previous,
    vscode.ConfigurationTarget.Workspace,
  )
  const extraEnv = {
    ...raConfig.get<Record<string, unknown>>("server.extraEnv", {}),
  }
  delete extraEnv["TYPEWELD_RUN_LSP"]
  delete extraEnv["TYPEWELD_RUST_ANALYZER"]
  await raConfig.update(
    "server.extraEnv",
    Object.keys(extraEnv).length === 0 ? undefined : extraEnv,
    vscode.ConfigurationTarget.Workspace,
  )
  await context.workspaceState.update(declinedKey, true)
  log("rust-analyzer integration disabled; original settings restored.")
  void vscode.commands
    .executeCommand("rust-analyzer.restartServer")
    .then(undefined, () => {})
}

/** The rust-analyzer binary bundled inside the rust-analyzer extension. */
function bundledRustAnalyzer(): string | undefined {
  const extension = vscode.extensions.getExtension(rustAnalyzerExtensionId)
  if (extension === undefined) {
    return undefined
  }
  const name =
    process.platform === "win32" ? "rust-analyzer.exe" : "rust-analyzer"
  const candidate = path.join(extension.extensionPath, "server", name)
  return isExecutable(candidate) ? candidate : undefined
}

async function hasTypeweldWorkspace(): Promise<boolean> {
  const folders = vscode.workspace.workspaceFolders ?? []
  if (folders.length === 0) {
    return false
  }
  if (folders.some((folder) => hasMarkerInOrAbove(folder.uri.fsPath))) {
    return true
  }

  const nested = await vscode.workspace.findFiles(
    `**/${workspaceMarker}`,
    "**/node_modules/**",
    1,
  )
  return nested.length > 0
}

/** The directory containing `typeweld.toml`, from the workspace folders. */
function typeweldRoot(): string | undefined {
  for (const folder of vscode.workspace.workspaceFolders ?? []) {
    let current = path.resolve(folder.uri.fsPath)
    while (true) {
      if (fs.existsSync(path.join(current, workspaceMarker))) {
        return current
      }
      const parent = path.dirname(current)
      if (parent === current) break
      current = parent
    }
  }
  return undefined
}

function hasMarkerInOrAbove(startDir: string): boolean {
  let current = path.resolve(startDir)
  while (true) {
    if (fs.existsSync(path.join(current, workspaceMarker))) {
      return true
    }
    const parent = path.dirname(current)
    if (parent === current) {
      return false
    }
    current = parent
  }
}

async function startClient(context: vscode.ExtensionContext): Promise<void> {
  const config = vscode.workspace.getConfiguration("typeweld")
  const binary = resolveServerBinary(context, config)
  if (binary === undefined) {
    log(
      [
        "Could not find the typeweld binary.",
        "Set typeweld.server.path, export TYPEWELD_LS_BINARY, add typeweld to PATH,",
        "or build it with `cargo build -p typeweld-cli --bin typeweld`.",
      ].join(" "),
    )
    void vscode.window.showWarningMessage(
      "Typeweld: could not find the typeweld binary. See the Typeweld output for details.",
    )
    return
  }

  const args = ["lsp", ...config.get<string[]>("server.args", [])]
  const serverOptions: ServerOptions = {
    command: binary,
    args,
    options: { env: process.env },
  }
  const clientOptions: LanguageClientOptions = {
    documentSelector,
    outputChannel: outputChannel!,
    revealOutputChannelOn: RevealOutputChannelOn.Never,
  }

  const newClient = new LanguageClient(
    "typeweld",
    "Typeweld",
    serverOptions,
    clientOptions,
  )
  client = newClient
  log(`Starting \`${binary} ${args.join(" ")}\`.`)
  try {
    await newClient.start()
    log("Typeweld language server is running.")
  } catch (error) {
    if (client === newClient) {
      client = undefined
    }
    log(`Typeweld language server failed to start: ${formatError(error)}`)
    void vscode.window.showWarningMessage(
      "Typeweld: the language server failed to start. See the Typeweld output for details.",
    )
  }
}

async function stopClient(): Promise<void> {
  const current = client
  client = undefined
  if (current === undefined) {
    return
  }
  try {
    await current.stop()
  } catch (error) {
    log(`Failed to stop the language server: ${formatError(error)}`)
  }
}

function resolveServerBinary(
  context: vscode.ExtensionContext,
  config: vscode.WorkspaceConfiguration,
): string | undefined {
  const configured = config.get<string>("server.path", "").trim()
  if (configured !== "") {
    return configured
  }

  const override = process.env.TYPEWELD_LS_BINARY?.trim()
  if (override !== undefined && override !== "") {
    return override
  }

  const onPath = findOnPath()
  if (onPath !== undefined) {
    return onPath
  }

  const bundled = bundledTypeweldBinary(context)
  if (bundled !== undefined) {
    return bundled
  }

  return cargoTargetCandidates(context).find(isExecutable)
}

/** The platform binary release builds bundle into the extension. */
function bundledTypeweldBinary(
  context: vscode.ExtensionContext,
): string | undefined {
  const candidate = path.join(
    context.extensionPath,
    "bin",
    `${process.platform}-${process.arch}`,
    executableName,
  )
  return isExecutable(candidate) ? candidate : undefined
}

function findOnPath(): string | undefined {
  const entries = (process.env.PATH ?? "")
    .split(path.delimiter)
    .filter((entry) => entry !== "")
  for (const entry of entries) {
    const candidate = path.resolve(entry, executableName)
    if (isExecutable(candidate)) {
      return candidate
    }
  }
  return undefined
}

function cargoTargetCandidates(context: vscode.ExtensionContext): string[] {
  const roots = (vscode.workspace.workspaceFolders ?? []).map(
    (folder) => folder.uri.fsPath,
  )
  roots.push(path.resolve(context.extensionPath, "..", ".."))

  return roots.flatMap((root) => [
    path.join(root, "target", "debug", executableName),
    path.join(root, "target", "release", executableName),
  ])
}

function isExecutable(candidate: string): boolean {
  try {
    if (!fs.statSync(candidate).isFile()) {
      return false
    }
    fs.accessSync(
      candidate,
      process.platform === "win32" ? fs.constants.F_OK : fs.constants.X_OK,
    )
    return true
  } catch {
    return false
  }
}

function log(message: string): void {
  outputChannel?.appendLine(message)
}

function formatError(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
