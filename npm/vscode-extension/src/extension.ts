import * as fs from "node:fs"
import { createRequire } from "node:module"
import * as path from "node:path"

import * as vscode from "vscode"
import {
  LanguageClient,
  RevealOutputChannelOn,
  State,
  TransportKind,
  type LanguageClientOptions,
  type ServerOptions,
} from "vscode-languageclient/node"

const apiLsOpenGeneratedFileCommand = "api-ls.openGeneratedPackageFile"
const languageClientId = "rustTsIntegration.apiLs"
const supportedLanguages = [
  "rust",
  "typescript",
  "typescriptreact",
  "javascript",
  "javascriptreact",
]

const nodeRequire = createRequire(__filename)

type ClientEntry = {
  client: LanguageClient
  disposables: vscode.Disposable[]
}

const clients = new Map<string, ClientEntry>()
let extensionContext: vscode.ExtensionContext | undefined
let outputChannel: vscode.LogOutputChannel | undefined
let reconcileQueue = Promise.resolve()

export function activate(context: vscode.ExtensionContext): void {
  extensionContext = context
  outputChannel = vscode.window.createOutputChannel("Rust TS Integration", {
    log: true,
  })

  context.subscriptions.push(
    outputChannel,
    vscode.commands.registerCommand(
      "rustTsIntegration.apiLs.restart",
      restartClients,
    ),
    vscode.workspace.onDidChangeWorkspaceFolders(() => {
      queueReconcile()
    }),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("rustTsIntegration.apiLs")) {
        void restartClients()
      }
    }),
  )

  queueReconcile()
}

export async function deactivate(): Promise<void> {
  extensionContext = undefined
  await stopAllClients()
}

function queueReconcile(): void {
  const context = extensionContext
  if (context === undefined) {
    return
  }

  reconcileQueue = reconcileQueue
    .then(() => reconcileClients(context))
    .catch((error: unknown) => {
      log(`api-ls client reconciliation failed: ${formatError(error)}`)
    })
}

async function reconcileClients(context: vscode.ExtensionContext): Promise<void> {
  if (extensionContext !== context) {
    return
  }

  const folders = vscode.workspace.workspaceFolders ?? []
  const wantedFolders = folders.filter(shouldStartForFolder)
  const wantedKeys = new Set(wantedFolders.map(folderKey))

  for (const [key, entry] of clients) {
    if (!wantedKeys.has(key)) {
      clients.delete(key)
      await stopClient(entry)
    }
  }

  for (const folder of wantedFolders) {
    const key = folderKey(folder)
    if (!clients.has(key)) {
      await startClient(context, folder)
    }
  }

  if (wantedFolders.length === 0) {
    log(
      "No api-ls workspace markers found. Configure rustTsIntegration.apiLs.requiredWorkspaceMarkers to change startup detection.",
    )
  }
}

async function startClient(
  context: vscode.ExtensionContext,
  folder: vscode.WorkspaceFolder,
): Promise<void> {
  const watcher = vscode.workspace.createFileSystemWatcher(
    new vscode.RelativePattern(
      folder,
      "{.api-ls.json,api-ls.json,Cargo.toml,package.json}",
    ),
  )
  const client = new LanguageClient(
    `${languageClientId}.${folder.index}`,
    `Rust TS Integration (${folder.name})`,
    serverOptions(context, folder),
    clientOptions(folder, watcher),
  )
  const stateListener = client.onDidChangeState((event) => {
    if (event.newState === State.Running) {
      log(`api-ls started for ${folder.name}`)
    } else if (event.newState === State.Stopped) {
      log(`api-ls stopped for ${folder.name}`)
    }
  })
  const entry: ClientEntry = {
    client,
    disposables: [watcher, stateListener],
  }

  clients.set(folderKey(folder), entry)
  try {
    await client.start()
  } catch (error) {
    clients.delete(folderKey(folder))
    disposeAll(entry.disposables)
    log(`api-ls failed to start for ${folder.name}: ${formatError(error)}`)
    void vscode.window.showWarningMessage(
      `api-ls failed to start for ${folder.name}. See the Rust TS Integration output for details.`,
    )
  }
}

function clientOptions(
  folder: vscode.WorkspaceFolder,
  watcher: vscode.FileSystemWatcher,
): LanguageClientOptions {
  return {
    documentSelector: supportedLanguages.map((language) => ({
      language,
      pattern: workspaceGlob(folder),
    })),
    initializationFailedHandler: (error) => {
      log(`api-ls initialization failed for ${folder.name}: ${formatError(error)}`)
      return false
    },
    middleware: {
      executeCommand: async (command, args, next) => {
        const result = await next(command, args)
        if (command === apiLsOpenGeneratedFileCommand) {
          await openGeneratedPackageFile(result)
        }
        return result
      },
    },
    outputChannel: outputChannel!,
    revealOutputChannelOn: RevealOutputChannelOn.Never,
    synchronize: {
      fileEvents: watcher,
    },
    workspaceFolder: folder,
  }
}

function serverOptions(
  context: vscode.ExtensionContext,
  folder: vscode.WorkspaceFolder,
): ServerOptions {
  const config = vscode.workspace.getConfiguration(
    "rustTsIntegration.apiLs",
    folder.uri,
  )
  const command = config.get<string>("command", "").trim()
  const args = config.get<string[]>("args", [])
  const options = {
    cwd: folder.uri.fsPath,
    env: mergedEnvironment(config),
  }

  if (command !== "") {
    return {
      debug: { command, args, options },
      run: { command, args, options },
    }
  }

  const launcher = bundledLauncherPath(context)
  if (launcher === undefined) {
    log("Bundled api-ls launcher was not found; falling back to api-ls on PATH.")
    return {
      debug: { command: "api-ls", args, options },
      run: { command: "api-ls", args, options },
    }
  }

  return {
    debug: {
      module: launcher,
      transport: TransportKind.stdio,
      args,
      options,
    },
    run: {
      module: launcher,
      transport: TransportKind.stdio,
      args,
      options,
    },
  }
}

function shouldStartForFolder(folder: vscode.WorkspaceFolder): boolean {
  const config = vscode.workspace.getConfiguration(
    "rustTsIntegration.apiLs",
    folder.uri,
  )
  if (!config.get<boolean>("enabled", true)) {
    return false
  }

  const markers = config.get<string[]>("requiredWorkspaceMarkers", [
    ".api-ls.json",
    "api-ls.json",
    "target/api-contract/effect-v4/packages",
  ])
  if (markers.length === 0) {
    return true
  }

  return hasWorkspaceMarker(folder.uri.fsPath, markers)
}

function hasWorkspaceMarker(startDir: string, markers: readonly string[]): boolean {
  let current = path.resolve(startDir)

  while (true) {
    if (
      markers.some((marker) => {
        if (marker.trim() === "") {
          return false
        }
        return fs.existsSync(path.join(current, marker))
      })
    ) {
      return true
    }

    const parent = path.dirname(current)
    if (parent === current) {
      return false
    }
    current = parent
  }
}

function bundledLauncherPath(context: vscode.ExtensionContext): string | undefined {
  const packagedLauncher = context.asAbsolutePath(
    path.join("server", "index.js"),
  )
  if (fs.existsSync(packagedLauncher)) {
    return packagedLauncher
  }

  try {
    return nodeRequire.resolve("@rust-ts-integration/language-server/src/index.js")
  } catch {
    const packagedPath = context.asAbsolutePath(
      path.join(
        "node_modules",
        "@rust-ts-integration",
        "language-server",
        "src",
        "index.js",
      ),
    )
    return fs.existsSync(packagedPath) ? packagedPath : undefined
  }
}

function mergedEnvironment(
  config: vscode.WorkspaceConfiguration,
): NodeJS.ProcessEnv {
  const env = { ...process.env }
  const configuredEnv = config.get<Record<string, string>>("env", {})

  for (const [key, value] of Object.entries(configuredEnv)) {
    env[key] = value
  }

  return env
}

async function restartClients(): Promise<void> {
  const context = extensionContext
  log("Restarting api-ls clients.")
  reconcileQueue = reconcileQueue
    .then(async () => {
      await stopAllClients()
      if (context !== undefined && extensionContext === context) {
        await reconcileClients(context)
      }
    })
    .catch((error: unknown) => {
      log(`api-ls restart failed: ${formatError(error)}`)
    })
  await reconcileQueue
}

async function stopAllClients(): Promise<void> {
  const entries = [...clients.values()]
  clients.clear()
  await Promise.all(entries.map(stopClient))
}

async function stopClient(entry: ClientEntry): Promise<void> {
  disposeAll(entry.disposables)
  await entry.client.stop()
}

function disposeAll(disposables: readonly vscode.Disposable[]): void {
  for (const disposable of disposables) {
    disposable.dispose()
  }
}

async function openGeneratedPackageFile(result: unknown): Promise<void> {
  if (!isObject(result) || typeof result.uri !== "string") {
    return
  }

  const document = await vscode.workspace.openTextDocument(
    vscode.Uri.parse(result.uri),
  )
  await vscode.window.showTextDocument(document, { preview: false })
}

function folderKey(folder: vscode.WorkspaceFolder): string {
  return folder.uri.toString()
}

function workspaceGlob(folder: vscode.WorkspaceFolder): string {
  return `${folder.uri.fsPath.replace(/\\/g, "/")}/**/*`
}

function log(message: string): void {
  outputChannel?.appendLine(message)
}

function formatError(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function isObject(value: unknown): value is { uri?: unknown } {
  return typeof value === "object" && value !== null
}
