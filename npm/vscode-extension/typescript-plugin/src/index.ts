// The typeweld TypeScript server plugin.
//
// Runs inside the user's own tsserver (VS Code's built-in TypeScript,
// typescript-language-server, vtsls, …) and connects back to the typeweld
// language server through the discovery file `target/typeweld/ls.json`.
//
// The daemon pushes a snapshot of every generated-file mark (with its Rust
// target and hover text); all synchronous language-service hooks answer from
// that local snapshot — no I/O ever happens inside a hook. Over the same
// socket the daemon asks for semantic rename locations (the property
// accesses a Rust-initiated field rename must cover), answered on the node
// event loop by calling the underlying LanguageService.
//
// TypeScript-initiated renames of API symbols cannot carry Rust edits
// through the tsserver protocol (the new name never reaches tsserver), so
// the plugin filters the generated-file locations out of the editor's
// rename, remembers where the rename will land, and watches the project for
// the edit to apply; the observed new name is reported to the daemon, which
// performs the Rust complement via its own rust-analyzer.

"use strict"

import * as fs from "node:fs"
import * as net from "node:net"
import * as path from "node:path"

const logPrefix = "[typeweld]"

// --- Minimal structural types (the plugin has no typescript dependency) ---

type TextSpan = { start: number; length: number }

type DefinitionInfo = {
  fileName: string
  textSpan: TextSpan
  kind: string
  name: string
  containerKind: string
  containerName: string
  [key: string]: unknown
}

type DefinitionAndBoundSpan = {
  definitions?: readonly DefinitionInfo[]
  textSpan: TextSpan
  [key: string]: unknown
}

type ReferenceEntry = {
  fileName: string
  textSpan: TextSpan
  isWriteAccess?: boolean
  [key: string]: unknown
}

type ReferencedSymbol = {
  definition: { fileName: string; textSpan?: TextSpan; [key: string]: unknown }
  references: readonly ReferenceEntry[]
  [key: string]: unknown
}

type RenameLocation = {
  fileName: string
  textSpan: TextSpan
  [key: string]: unknown
}

type QuickInfo = {
  kind: string
  kindModifiers: string
  textSpan: TextSpan
  displayParts?: readonly { text: string; kind: string }[]
  documentation?: readonly { text: string; kind: string }[]
  [key: string]: unknown
}

type SourceFileLike = { text: string }

type Program = {
  getSourceFile(fileName: string): SourceFileLike | undefined
}

type LanguageService = Record<string, unknown> & {
  getDefinitionAtPosition?(
    fileName: string,
    position: number,
  ): readonly DefinitionInfo[] | undefined
  getDefinitionAndBoundSpan?(
    fileName: string,
    position: number,
  ): DefinitionAndBoundSpan | undefined
  getReferencesAtPosition?(
    fileName: string,
    position: number,
  ): readonly ReferenceEntry[] | undefined
  findReferences?(
    fileName: string,
    position: number,
  ): readonly ReferencedSymbol[] | undefined
  getQuickInfoAtPosition?(
    fileName: string,
    position: number,
    maximumLength?: number,
  ): QuickInfo | undefined
  findRenameLocations?(
    fileName: string,
    position: number,
    findInStrings: boolean,
    findInComments: boolean,
    preferences?: unknown,
  ): readonly RenameLocation[] | undefined
  getProgram?(): Program | undefined
}

type PluginCreateInfo = {
  languageService: LanguageService
  project: {
    getCurrentDirectory(): string
    projectService: {
      logger: { info(message: string): void }
    }
  }
}

// --- Snapshot pushed by the daemon ---

type Mark = {
  file: string
  start: number
  end: number
  symbol: string
  kind: string
  target: { file: string; start: number; end: number }
  hover?: string | null
}

type Snapshot = {
  generation: number
  generatedDirs: string[]
  marks: Mark[]
}

/** A rename the editor is about to apply, watched for the new name. */
type PendingRename = {
  symbol: string
  oldName: string
  /** Per user file: the smallest span start (invariant under the edit). */
  probes: { fileName: string; start: number }[]
  startedAt: number
}

class Daemon {
  private snapshot: Snapshot | undefined
  private socket: net.Socket | undefined
  private buffer = ""
  private pending: PendingRename | undefined
  private readonly timer: ReturnType<typeof setInterval>

  constructor(private readonly info: PluginCreateInfo) {
    this.connect()
    this.timer = setInterval(() => {
      if (this.socket === undefined) {
        this.connect()
      }
      this.checkPendingRename()
    }, 1000)
    // Never keep tsserver alive on our account.
    this.timer.unref?.()
  }

  /** The smallest mark covering `position` in a generated file. */
  markAt(fileName: string, position: number): Mark | undefined {
    const file = normalize(fileName)
    let best: Mark | undefined
    for (const mark of this.snapshot?.marks ?? []) {
      if (normalize(mark.file) !== file) continue
      if (mark.start > position || position > mark.end) continue
      if (best === undefined || mark.end - mark.start < best.end - best.start) {
        best = mark
      }
    }
    return best
  }

  isGenerated(fileName: string): boolean {
    const file = normalize(fileName)
    return (this.snapshot?.generatedDirs ?? []).some((dir) =>
      file.startsWith(normalize(dir) + "/"),
    )
  }

  /**
   * Remembers a rename of an API symbol the editor is about to apply, so the
   * observed new name can be reported for the Rust complement.
   */
  watchRename(
    symbol: string,
    oldName: string,
    locations: readonly RenameLocation[],
  ): void {
    const starts = new Map<string, number>()
    for (const location of locations) {
      const file = normalize(location.fileName)
      const known = starts.get(file)
      if (known === undefined || location.textSpan.start < known) {
        starts.set(file, location.textSpan.start)
      }
    }
    if (starts.size === 0) return
    this.pending = {
      symbol,
      oldName,
      probes: [...starts].map(([fileName, start]) => ({ fileName, start })),
      startedAt: Date.now(),
    }
  }

  private checkPendingRename(): void {
    const pending = this.pending
    if (pending === undefined) return
    if (Date.now() - pending.startedAt > 30_000) {
      this.pending = undefined
      return
    }
    const program = this.info.languageService.getProgram?.()
    if (program === undefined) return
    for (const probe of pending.probes) {
      const source = program.getSourceFile(probe.fileName)
      if (source === undefined) continue
      const found = /^[A-Za-z0-9_$]+/.exec(source.text.slice(probe.start))
      const name = found?.[0]
      if (name !== undefined && name !== pending.oldName) {
        this.pending = undefined
        this.send({
          method: "renameLanded",
          symbol: pending.symbol,
          newName: name,
          // The user files the editor's own rename edited; the daemon asks
          // the editor extension to save them along with the Rust half.
          files: pending.probes.map((probe) => probe.fileName),
        })
        return
      }
    }
  }

  private connect(): void {
    const discovered = discover(this.info.project.getCurrentDirectory())
    if (discovered === undefined) return
    const socket = net.connect(discovered.port, "127.0.0.1")
    socket.setNoDelay(true)
    socket.unref()
    socket.on("connect", () => {
      this.socket = socket
      this.buffer = ""
      socket.write(
        JSON.stringify({
          method: "hello",
          kind: "tsplugin",
          token: discovered.token,
          pid: process.pid,
        }) + "\n",
      )
      this.log("connected to typeweld-ls")
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
        // Malformed daemon message: ignore, fail open.
      }
    }
  }

  private onMessage(message: Record<string, unknown>): void {
    if (message["method"] === "snapshot") {
      this.snapshot = message as unknown as Snapshot
      return
    }
    if (message["method"] === "query" && typeof message["id"] === "number") {
      const result = this.answerQuery(message)
      this.send({ id: message["id"], result })
    }
  }

  /** Semantic queries answered with the underlying LanguageService. */
  private answerQuery(query: Record<string, unknown>): unknown {
    if (
      query["kind"] === "renameLocations" &&
      typeof query["file"] === "string" &&
      typeof query["position"] === "number"
    ) {
      try {
        const locations = this.info.languageService.findRenameLocations?.(
          query["file"],
          query["position"],
          false,
          false,
          { providePrefixAndSuffixTextForRename: false },
        )
        return (locations ?? []).map((location) => ({
          file: location.fileName,
          start: location.textSpan.start,
          length: location.textSpan.length,
        }))
      } catch {
        return []
      }
    }
    return null
  }

  private send(message: unknown): void {
    try {
      this.socket?.write(JSON.stringify(message) + "\n")
    } catch {
      // Connection raced shut; the retry timer reconnects.
    }
  }

  private log(message: string): void {
    try {
      this.info.project.projectService.logger.info(`${logPrefix} ${message}`)
    } catch {
      // Logging must never break the host.
    }
  }
}

/** Finds `target/typeweld/ls.json` by walking up from `start`. */
function discover(start: string): { port: number; token: string } | undefined {
  let dir = path.resolve(start)
  for (let depth = 0; depth < 16; depth++) {
    const candidate = path.join(dir, "target", "typeweld", "ls.json")
    try {
      const parsed = JSON.parse(fs.readFileSync(candidate, "utf8")) as {
        port?: number
        token?: string
      }
      if (typeof parsed.port === "number" && typeof parsed.token === "string") {
        return { port: parsed.port, token: parsed.token }
      }
    } catch {
      // Keep walking.
    }
    const parent = path.dirname(dir)
    if (parent === dir) break
    dir = parent
  }
  return undefined
}

function normalize(fileName: string): string {
  const slashes = fileName.replace(/\\/g, "/")
  // Windows drive letters compare case-insensitively; tsserver reports them
  // lowercase, the daemon may not.
  return /^[A-Za-z]:\//.test(slashes)
    ? slashes[0]!.toLowerCase() + slashes.slice(1)
    : slashes
}

function identifierAt(text: string, span: TextSpan): string {
  return text.slice(span.start, span.start + span.length)
}

function init() {
  return {
    create(info: PluginCreateInfo): LanguageService {
      const languageService = info.languageService
      const daemon = new Daemon(info)

      const proxy: LanguageService = Object.create(null)
      for (const key of Object.keys(languageService)) {
        const value = languageService[key]
        proxy[key] =
          typeof value === "function" ? value.bind(languageService) : value
      }

      const sourceText = (fileName: string): string | undefined =>
        languageService.getProgram?.()?.getSourceFile(fileName)?.text

      /** Maps a definition that landed in a generated file to its Rust target. */
      const remapDefinition = (
        definition: DefinitionInfo,
      ): DefinitionInfo | undefined => {
        if (!daemon.isGenerated(definition.fileName)) return definition
        const mark = daemon.markAt(definition.fileName, definition.textSpan.start)
        if (mark === undefined) return undefined
        return {
          ...definition,
          fileName: mark.target.file,
          textSpan: {
            start: mark.target.start,
            length: mark.target.end - mark.target.start,
          },
          containerKind: "module",
          containerName: "rust",
        }
      }

      const remapDefinitions = (
        definitions: readonly DefinitionInfo[] | undefined,
      ): DefinitionInfo[] | undefined => {
        if (definitions === undefined) return undefined
        const remapped: DefinitionInfo[] = []
        for (const definition of definitions) {
          const mapped = remapDefinition(definition)
          if (mapped !== undefined) remapped.push(mapped)
        }
        return remapped.length === 0 && definitions.length > 0
          ? undefined
          : remapped
      }

      if (languageService.getDefinitionAtPosition !== undefined) {
        const prior = languageService.getDefinitionAtPosition.bind(languageService)
        proxy.getDefinitionAtPosition = (fileName, position) =>
          remapDefinitions(prior(fileName, position))
      }

      if (languageService.getDefinitionAndBoundSpan !== undefined) {
        const prior = languageService.getDefinitionAndBoundSpan.bind(languageService)
        proxy.getDefinitionAndBoundSpan = (fileName, position) => {
          // Inside a generated file, the mark itself answers directly.
          if (daemon.isGenerated(fileName)) {
            const own = daemon.markAt(fileName, position)
            if (own !== undefined) {
              return {
                textSpan: { start: own.start, length: own.end - own.start },
                definitions: [
                  {
                    fileName: own.target.file,
                    textSpan: {
                      start: own.target.start,
                      length: own.target.end - own.target.start,
                    },
                    kind: "function",
                    name: own.symbol,
                    containerKind: "module",
                    containerName: "rust",
                  },
                ],
              }
            }
          }
          const result = prior(fileName, position)
          if (result === undefined || result.definitions === undefined) {
            return result
          }
          const definitions = remapDefinitions(result.definitions)
          if (definitions === undefined) return undefined
          return { ...result, definitions }
        }
      }

      if (languageService.getReferencesAtPosition !== undefined) {
        const prior = languageService.getReferencesAtPosition.bind(languageService)
        proxy.getReferencesAtPosition = (fileName, position) => {
          const references = prior(fileName, position)
          if (references === undefined) return undefined
          const kept: ReferenceEntry[] = []
          const rust: ReferenceEntry[] = []
          for (const reference of references) {
            if (!daemon.isGenerated(reference.fileName)) {
              kept.push(reference)
              continue
            }
            const mark = daemon.markAt(reference.fileName, reference.textSpan.start)
            if (mark !== undefined) {
              rust.push({
                fileName: mark.target.file,
                textSpan: {
                  start: mark.target.start,
                  length: mark.target.end - mark.target.start,
                },
              })
            }
          }
          const merged = [...kept, ...dedupe(rust)]
          return merged.length === 0 ? undefined : merged
        }
      }

      if (languageService.findReferences !== undefined) {
        const prior = languageService.findReferences.bind(languageService)
        proxy.findReferences = (fileName, position) => {
          const symbols = prior(fileName, position)
          if (symbols === undefined) return undefined
          const kept = symbols.flatMap((symbol): ReferencedSymbol[] => {
            const references = symbol.references.filter(
              (reference) => !daemon.isGenerated(reference.fileName),
            )
            if (daemon.isGenerated(symbol.definition.fileName)) {
              const mark =
                symbol.definition.textSpan === undefined
                  ? undefined
                  : daemon.markAt(
                      symbol.definition.fileName,
                      symbol.definition.textSpan.start,
                    )
              if (mark === undefined) {
                return references.length === 0 ? [] : [{ ...symbol, references }]
              }
              return [
                {
                  ...symbol,
                  definition: {
                    ...symbol.definition,
                    fileName: mark.target.file,
                    textSpan: {
                      start: mark.target.start,
                      length: mark.target.end - mark.target.start,
                    },
                  },
                  references,
                },
              ]
            }
            return references.length === symbol.references.length
              ? [symbol]
              : [{ ...symbol, references }]
          })
          return kept.length === 0 ? undefined : kept
        }
      }

      if (languageService.getQuickInfoAtPosition !== undefined) {
        const prior = languageService.getQuickInfoAtPosition.bind(languageService)
        proxy.getQuickInfoAtPosition = (fileName, position, maximumLength) => {
          const result = prior(fileName, position, maximumLength)
          // Contract hover for marks (in generated files, or where the
          // definition of the symbol under the cursor is a generated mark).
          let mark = daemon.isGenerated(fileName)
            ? daemon.markAt(fileName, position)
            : undefined
          if (mark === undefined) {
            const definitions = languageService.getDefinitionAtPosition?.(
              fileName,
              position,
            )
            for (const definition of definitions ?? []) {
              if (daemon.isGenerated(definition.fileName)) {
                mark = daemon.markAt(definition.fileName, definition.textSpan.start)
                if (mark !== undefined) break
              }
            }
          }
          const hover = mark?.hover
          if (hover === undefined || hover === null || hover === "") return result
          const documentation = [
            ...(result?.documentation ?? []),
            { text: hover, kind: "text" },
          ]
          if (result !== undefined) return { ...result, documentation }
          return {
            kind: "alias",
            kindModifiers: "",
            textSpan:
              mark !== undefined && daemon.isGenerated(fileName)
                ? { start: mark.start, length: mark.end - mark.start }
                : { start: position, length: 1 },
            displayParts: [],
            documentation,
          }
        }
      }

      if (languageService.findRenameLocations !== undefined) {
        const prior = languageService.findRenameLocations.bind(languageService)
        proxy.findRenameLocations = (
          fileName,
          position,
          findInStrings,
          findInComments,
          preferences,
        ) => {
          const locations = prior(
            fileName,
            position,
            findInStrings,
            findInComments,
            preferences,
          )
          if (locations === undefined) return undefined
          let mark: Mark | undefined
          const user: RenameLocation[] = []
          for (const location of locations) {
            if (daemon.isGenerated(location.fileName)) {
              mark ??= daemon.markAt(location.fileName, location.textSpan.start)
            } else {
              user.push(location)
            }
          }
          if (mark === undefined) return locations
          // An API symbol: the editor edits only user files (generated files
          // regenerate), and the daemon owes the Rust complement once the
          // edit lands.
          if (user.length === 0) return undefined
          const probe = user[0]
          const text = probe === undefined ? undefined : sourceText(probe.fileName)
          const oldName =
            probe !== undefined && text !== undefined
              ? identifierAt(text, probe.textSpan)
              : undefined
          if (oldName !== undefined && oldName !== "") {
            daemon.watchRename(mark.symbol, oldName, user)
          }
          return user
        }
      }

      try {
        info.project.projectService.logger.info(`${logPrefix} plugin active`)
      } catch {
        // Logging must never break the host.
      }
      return proxy
    },
  }
}

function dedupe(references: ReferenceEntry[]): ReferenceEntry[] {
  const seen = new Set<string>()
  return references.filter((reference) => {
    const key = `${reference.fileName}:${reference.textSpan.start}:${reference.textSpan.length}`
    if (seen.has(key)) return false
    seen.add(key)
    return true
  })
}

export = init
