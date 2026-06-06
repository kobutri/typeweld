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
const privateTypeScriptCommandPrefix = "_typescript."
const reservedTypeScriptCommands = new Set(["typescript.tsserverRequest"])
const directNavigationMethods = new Set([
  "textDocument/definition",
  "textDocument/hover",
  "textDocument/references",
])
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

type ApiClientDocumentSelector = Array<{
  scheme: "file"
  language: string
  pattern: string
}>

type ApiDirectDocumentSelector = Array<{
  scheme: "file"
  language: string
  pattern: vscode.GlobPattern
}>

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
  sanitizeInitializeFeatures(client)
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
    entry.disposables.push(...registerDirectNavigationProviders(client, folder))
  } catch (error) {
    clients.delete(folderKey(folder))
    disposeAll(entry.disposables)
    log(`api-ls failed to start for ${folder.name}: ${formatError(error)}`)
    void vscode.window.showWarningMessage(
      `api-ls failed to start for ${folder.name}. See the Rust TS Integration output for details.`,
    )
  }
}

function registerDirectNavigationProviders(
  client: LanguageClient,
  folder: vscode.WorkspaceFolder,
): vscode.Disposable[] {
  const selector = directDocumentSelector(folder)

  const disposables = [
    vscode.languages.registerHoverProvider(selector, {
      provideHover: async (document, position, token) => {
        try {
          const result = await client.sendRequest(
            "textDocument/hover",
            textDocumentPositionParams(document, position),
          )
          if (token.isCancellationRequested) {
            return undefined
          }
          return lspHover(result)
        } catch (error) {
          log(`api-ls direct hover failed: ${formatError(error)}`)
          return undefined
        }
      },
    }),
    vscode.languages.registerDefinitionProvider(selector, {
      provideDefinition: async (document, position, token) => {
        try {
          const result = await client.sendRequest(
            "textDocument/definition",
            textDocumentPositionParams(document, position),
          )
          if (token.isCancellationRequested) {
            return undefined
          }
          return lspLocations(result, folder)
        } catch (error) {
          log(`api-ls direct definition failed: ${formatError(error)}`)
          return undefined
        }
      },
    }),
    vscode.languages.registerReferenceProvider(selector, {
      provideReferences: async (document, position, context, token) => {
        try {
          const result = await client.sendRequest(
            "textDocument/references",
            {
              ...textDocumentPositionParams(document, position),
              context: {
                includeDeclaration: context.includeDeclaration,
              },
            },
          )
          if (token.isCancellationRequested) {
            return undefined
          }
          return lspLocations(result, folder)
        } catch (error) {
          log(`api-ls direct references failed: ${formatError(error)}`)
          return undefined
        }
      },
    }),
  ]
  log(`Registered direct api-ls navigation providers for ${folder.name} with workspace-relative selectors.`)
  return disposables
}

function textDocumentPositionParams(
  document: vscode.TextDocument,
  position: vscode.Position,
): {
  textDocument: { uri: string }
  position: { line: number; character: number }
} {
  return {
    textDocument: {
      uri: document.uri.toString(),
    },
    position: {
      line: position.line,
      character: position.character,
    },
  }
}

function lspLocations(
  result: unknown,
  folder: vscode.WorkspaceFolder,
): vscode.Location[] | undefined {
  if (Array.isArray(result)) {
    return result.flatMap((location) => {
      const converted = lspLocation(location)
      return converted === undefined || isGeneratedPackageLocation(converted, folder)
        ? []
        : [converted]
    })
  }

  const converted = lspLocation(result)
  return converted === undefined || isGeneratedPackageLocation(converted, folder)
    ? undefined
    : [converted]
}

function lspLocation(value: unknown): vscode.Location | undefined {
  if (!isRecord(value) || typeof value.uri !== "string" || !isRecord(value.range)) {
    return undefined
  }
  const range = lspRange(value.range)
  if (range === undefined) {
    return undefined
  }

  return new vscode.Location(vscode.Uri.parse(value.uri), range)
}

function isGeneratedPackageLocation(
  location: vscode.Location,
  folder: vscode.WorkspaceFolder,
): boolean {
  if (location.uri.scheme !== "file") {
    return false
  }

  const generatedRoot = path.join(
    folder.uri.fsPath,
    "target",
    "api-contract",
    "effect-v4",
    "packages",
  )
  return location.uri.fsPath === generatedRoot || location.uri.fsPath.startsWith(`${generatedRoot}${path.sep}`)
}

function lspHover(result: unknown): vscode.Hover | undefined {
  if (!isRecord(result)) {
    return undefined
  }
  const contents = lspHoverContents(result.contents)
  if (contents.length === 0) {
    return undefined
  }

  const range = isRecord(result.range) ? lspRange(result.range) : undefined
  return new vscode.Hover(contents, range)
}

function lspHoverContents(contents: unknown): vscode.MarkdownString[] {
  if (typeof contents === "string") {
    return [new vscode.MarkdownString(contents)]
  }
  if (Array.isArray(contents)) {
    return contents.flatMap(lspHoverContents)
  }
  if (isRecord(contents)) {
    if (typeof contents.value === "string") {
      if (typeof contents.language === "string") {
        return [
          new vscode.MarkdownString(
            ["```" + contents.language, contents.value, "```"].join("\n"),
          ),
        ]
      }
      return [new vscode.MarkdownString(contents.value)]
    }
  }

  return []
}

function lspRange(value: Record<string, unknown>): vscode.Range | undefined {
  if (!isRecord(value.start) || !isRecord(value.end)) {
    return undefined
  }
  const start = lspPosition(value.start)
  const end = lspPosition(value.end)
  if (start === undefined || end === undefined) {
    return undefined
  }

  return new vscode.Range(start, end)
}

function lspPosition(value: Record<string, unknown>): vscode.Position | undefined {
  if (typeof value.line !== "number" || typeof value.character !== "number") {
    return undefined
  }
  return new vscode.Position(value.line, value.character)
}

function clientOptions(
  folder: vscode.WorkspaceFolder,
  watcher: vscode.FileSystemWatcher,
): LanguageClientOptions {
  return {
    documentSelector: documentSelector(folder),
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
      handleRegisterCapability: async (params, next) => {
        const sanitized = sanitizeRegisterCapabilityParams(params)
        if (sanitized.registrations.length === 0) {
          return
        }
        const callNext = next as unknown as (
          params: typeof sanitized,
        ) => Promise<void | Error>
        const result = await callNext(sanitized)
        if (result instanceof Error) {
          throw result
        }
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

function documentSelector(folder: vscode.WorkspaceFolder): ApiClientDocumentSelector {
  return supportedLanguages.map((language) => ({
    scheme: "file",
    language,
    pattern: workspaceGlob(folder),
  }))
}

function directDocumentSelector(folder: vscode.WorkspaceFolder): ApiDirectDocumentSelector {
  return supportedLanguages.map((language) => ({
    scheme: "file",
    language,
    pattern: new vscode.RelativePattern(folder, "**/*"),
  }))
}

type LanguageClientRuntime = {
  initializeFeatures?: (connection: unknown) => void
  _capabilities?: unknown
}

function sanitizeInitializeFeatures(client: LanguageClient): void {
  const mutableClient = client as unknown as LanguageClientRuntime
  const originalInitializeFeatures = mutableClient.initializeFeatures
  if (typeof originalInitializeFeatures !== "function") {
    log("Unable to patch api-ls command capabilities before feature initialization.")
    return
  }

  mutableClient.initializeFeatures = function initializeFeaturesWithSanitizedCommands(
    this: LanguageClientRuntime,
    connection: unknown,
  ): void {
    sanitizeApiLsClientCapabilities(this._capabilities)
    return originalInitializeFeatures.call(this, connection)
  }
}

function sanitizeApiLsClientCapabilities(capabilities: unknown): void {
  if (!isRecord(capabilities)) {
    return
  }

  sanitizeExecuteCommandProvider(capabilities)
  delete capabilities.definitionProvider
  delete capabilities.hoverProvider
  delete capabilities.referencesProvider
}

function sanitizeRegisterCapabilityParams<T extends { registrations: unknown[] }>(
  params: T,
): T {
  return {
    ...params,
    registrations: params.registrations
      .map(sanitizeRegistration)
      .filter((registration): registration is NonNullable<typeof registration> => {
        return registration !== undefined
      }),
  }
}

function sanitizeRegistration(registration: unknown): unknown | undefined {
  if (!isRecord(registration)) {
    return registration
  }
  if (
    typeof registration.method === "string" &&
    directNavigationMethods.has(registration.method)
  ) {
    return undefined
  }
  if (registration.method !== "workspace/executeCommand") {
    return registration
  }

  const registerOptions = isRecord(registration.registerOptions)
    ? { ...registration.registerOptions }
    : {}
  const commands = Array.isArray(registerOptions.commands)
    ? registerOptions.commands.filter((command) => {
        return typeof command !== "string" || !isReservedTypeScriptCommand(command)
      })
    : undefined

  if (commands === undefined) {
    return registration
  }
  if (commands.length === 0) {
    return undefined
  }

  return {
    ...registration,
    registerOptions: {
      ...registerOptions,
      commands,
    },
  }
}

function sanitizeExecuteCommandProvider(capabilities: unknown): void {
  if (!isRecord(capabilities)) {
    return
  }
  const provider = capabilities.executeCommandProvider
  if (!isRecord(provider) || !Array.isArray(provider.commands)) {
    return
  }

  provider.commands = provider.commands.filter((command) => {
    return typeof command !== "string" || !isReservedTypeScriptCommand(command)
  })
}

function isReservedTypeScriptCommand(command: string): boolean {
  return (
    command.startsWith(privateTypeScriptCommandPrefix) ||
    reservedTypeScriptCommands.has(command)
  )
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object"
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

  if (isTypeScriptLauncher(launcher)) {
    const tsx = bundledTsxPath(context)
    if (tsx === undefined) {
      log("TypeScript api-ls launcher was found but tsx was not found; falling back to api-ls on PATH.")
      return {
        debug: { command: "api-ls", args, options },
        run: { command: "api-ls", args, options },
      }
    }

    return {
      debug: {
        command: process.execPath,
        args: [tsx, launcher, ...args],
        options,
      },
      run: {
        command: process.execPath,
        args: [tsx, launcher, ...args],
        options,
      },
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

function isTypeScriptLauncher(launcher: string): boolean {
  return launcher.endsWith(".ts") || launcher.endsWith(".mts")
}

function bundledLauncherPath(context: vscode.ExtensionContext): string | undefined {
  const packagedLauncher = context.asAbsolutePath(
    path.join("server", "index.js"),
  )
  if (fs.existsSync(packagedLauncher)) {
    return packagedLauncher
  }

  try {
    return nodeRequire.resolve("@rust-ts-integration/language-server/src/index.ts")
  } catch {
    const packagedPath = context.asAbsolutePath(
      path.join(
        "node_modules",
        "@rust-ts-integration",
        "language-server",
        "src",
        "index.ts",
      ),
    )
    return fs.existsSync(packagedPath) ? packagedPath : undefined
  }
}

function bundledTsxPath(context: vscode.ExtensionContext): string | undefined {
  try {
    return nodeRequire.resolve("tsx/cli")
  } catch {
    const workspacePath = context.asAbsolutePath(
      path.join("..", "node_modules", "tsx", "dist", "cli.mjs"),
    )
    return fs.existsSync(workspacePath) ? workspacePath : undefined
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
