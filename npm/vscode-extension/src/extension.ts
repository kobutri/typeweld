import * as fs from "node:fs"
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
const documentSelector = [
  { scheme: "file", language: "rust" },
  { scheme: "file", language: "typescript" },
  { scheme: "file", language: "typescriptreact" },
]

let extensionContext: vscode.ExtensionContext | undefined
let outputChannel: vscode.LogOutputChannel | undefined
let client: LanguageClient | undefined
let reconcileQueue = Promise.resolve()

export function activate(context: vscode.ExtensionContext): void {
  extensionContext = context
  outputChannel = vscode.window.createOutputChannel("Typeweld", { log: true })

  context.subscriptions.push(
    outputChannel,
    vscode.commands.registerCommand("typeweld.server.restart", () => {
      queueReconcile({ restart: true })
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
      log(`No ${workspaceMarker} found in the workspace; stopping the language server.`)
      await stopClient()
    }
    return
  }
  if (client !== undefined) {
    return
  }

  await startClient(extensionContext)
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

  return cargoTargetCandidates(context).find(isExecutable)
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
