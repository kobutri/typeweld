import {
  copyFileSync,
  createWriteStream,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
} from "node:fs"
import { dirname, join, relative, resolve, sep } from "node:path"
import type { Readable } from "node:stream"
import { fileURLToPath } from "node:url"

import * as esbuild from "esbuild"
import * as yauzl from "yauzl"
import * as yazl from "yazl"

const scriptDir = dirname(fileURLToPath(import.meta.url))
const extensionRoot = resolve(scriptDir, "..")
const manifest = JSON.parse(
  readFileSync(join(extensionRoot, "package.json"), "utf8"),
)
const options = parseOptions(process.argv.slice(2))
const vsixPath =
  options.vsixPath ?? join(extensionRoot, `${manifest.name}-${manifest.version}.vsix`)
const outPath = options.outPath
const pluginSource = join(extensionRoot, "typescript-plugin")
const stagingRoot = join(extensionRoot, "out", "vsix-typescript-plugin")
const pluginTarget = join(
  stagingRoot,
  "extension",
  "node_modules",
  "@typeweld",
  "typescript-plugin",
)

assertFile(vsixPath, "VSIX")
assertDirectory(pluginSource, "TypeScript server plugin")

rmSync(stagingRoot, { force: true, recursive: true })
mkdirSync(dirname(pluginTarget), { recursive: true })
await buildTypescriptPlugin(pluginSource, pluginTarget)
await patchVsixDirectory(vsixPath, stagingRoot, "extension/node_modules")

console.log(`Patched TypeScript server plugin into ${vsixPath}`)

if (outPath !== undefined) {
  mkdirSync(dirname(outPath), { recursive: true })
  copyFileSync(vsixPath, outPath)
  console.log(`Wrote patched VSIX to ${outPath}`)
}

function parseOptions(args: readonly string[]): {
  readonly outPath: string | undefined
  readonly vsixPath: string | undefined
} {
  let outPath: string | undefined
  let vsixPath: string | undefined

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index]
    if (arg === undefined) {
      continue
    }
    if (arg === "--out") {
      const value = args[index + 1]
      if (value === undefined || value.startsWith("--")) {
        throw new Error("--out requires a path")
      }
      outPath = resolve(extensionRoot, value)
      index += 1
      continue
    }
    if (arg.startsWith("--out=")) {
      outPath = resolve(extensionRoot, arg.slice("--out=".length))
      continue
    }
    if (arg === "--vsix") {
      const value = args[index + 1]
      if (value === undefined || value.startsWith("--")) {
        throw new Error("--vsix requires a path")
      }
      vsixPath = resolve(extensionRoot, value)
      index += 1
      continue
    }
    if (arg.startsWith("--vsix=")) {
      vsixPath = resolve(extensionRoot, arg.slice("--vsix=".length))
      continue
    }
  }

  return { outPath, vsixPath }
}

function assertFile(path: string, label: string): void {
  try {
    if (statSync(path).isFile()) {
      return
    }
  } catch {
  }
  throw new Error(`Missing ${label}: ${path}`)
}

async function buildTypescriptPlugin(source: string, target: string): Promise<void> {
  const entry = join(source, "index.ts")
  const manifest = join(source, "package.json")
  assertFile(entry, "TypeScript server plugin source")
  assertFile(manifest, "TypeScript server plugin manifest")

  rmSync(target, { force: true, recursive: true })
  mkdirSync(target, { recursive: true })
  copyFileSync(manifest, join(target, "package.json"))
  await esbuild.build({
    bundle: false,
    entryPoints: [entry],
    format: "cjs",
    legalComments: "none",
    logLevel: "silent",
    outfile: join(target, "index.js"),
    platform: "node",
    target: "node20",
  })
}

async function patchVsixDirectory(
  targetVsixPath: string,
  sourceRoot: string,
  directory: string,
): Promise<void> {
  const sourceDirectory = join(sourceRoot, ...directory.split("/"))
  assertDirectory(sourceDirectory, "staged VSIX directory")

  const temporaryVsixPath = `${targetVsixPath}.tmp-${process.pid}`
  rmSync(temporaryVsixPath, { force: true })

  const inputZip = await openZip(targetVsixPath)
  const outputZip = new yazl.ZipFile()
  const writeStream = createWriteStream(temporaryVsixPath)
  const outputFinished = new Promise<void>((resolve, reject) => {
    outputZip.on("error", reject)
    writeStream.on("error", reject)
    writeStream.on("close", resolve)
  })

  outputZip.outputStream.pipe(writeStream)

  try {
    await copyZipEntriesExcept(inputZip, outputZip, `${directory}/`)
    addDirectoryToZip(outputZip, sourceRoot, sourceDirectory)
    outputZip.end()
    await outputFinished
  } catch (error) {
    rmSync(temporaryVsixPath, { force: true })
    throw error
  } finally {
    inputZip.close()
  }

  renameSync(temporaryVsixPath, targetVsixPath)
}

async function copyZipEntriesExcept(
  inputZip: yauzl.ZipFile,
  outputZip: yazl.ZipFile,
  skippedPrefix: string,
): Promise<void> {
  const entries = await readZipEntries(inputZip)
  for (const entry of entries) {
    if (entry.fileName === skippedPrefix || entry.fileName.startsWith(skippedPrefix)) {
      continue
    }

    const options = zipEntryOptions(entry)
    if (entry.fileName.endsWith("/")) {
      outputZip.addEmptyDirectory(entry.fileName, options)
      continue
    }

    const buffer = await readZipEntry(inputZip, entry)
    outputZip.addBuffer(buffer, entry.fileName, options)
  }
}

function zipEntryOptions(entry: yauzl.Entry): {
  readonly mtime: Date
  readonly mode?: number
} {
  const mode = entry.externalFileAttributes >>> 16
  if (mode === 0) {
    return { mtime: entry.getLastModDate() }
  }
  return { mode, mtime: entry.getLastModDate() }
}

function addDirectoryToZip(
  outputZip: yazl.ZipFile,
  sourceRoot: string,
  directory: string,
): void {
  for (const path of listFiles(directory)) {
    const metadataPath = relative(sourceRoot, path).split(sep).join("/")
    outputZip.addFile(path, metadataPath)
  }
}

function listFiles(directory: string): string[] {
  const files: string[] = []
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) {
      files.push(...listFiles(path))
    } else if (entry.isFile()) {
      files.push(path)
    }
  }
  return files
}

function openZip(path: string): Promise<yauzl.ZipFile> {
  return new Promise((resolve, reject) => {
    yauzl.open(path, { autoClose: false, lazyEntries: true }, (error, zipfile) => {
      if (error !== null) {
        reject(error)
      } else if (zipfile === undefined) {
        reject(new Error(`Failed to open VSIX: ${path}`))
      } else {
        resolve(zipfile)
      }
    })
  })
}

function readZipEntries(zipfile: yauzl.ZipFile): Promise<yauzl.Entry[]> {
  return new Promise((resolve, reject) => {
    const entries: yauzl.Entry[] = []
    zipfile.on("entry", (entry: yauzl.Entry) => {
      entries.push(entry)
      zipfile.readEntry()
    })
    zipfile.on("end", () => resolve(entries))
    zipfile.on("error", reject)
    zipfile.readEntry()
  })
}

function readZipEntry(
  zipfile: yauzl.ZipFile,
  entry: yauzl.Entry,
): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    zipfile.openReadStream(entry, (error, stream) => {
      if (error !== null) {
        reject(error)
        return
      }
      if (stream === undefined) {
        reject(new Error(`Failed to read VSIX entry: ${entry.fileName}`))
        return
      }

      collectStream(stream).then(resolve, reject)
    })
  })
}

async function collectStream(stream: Readable): Promise<Buffer> {
  const chunks: Buffer[] = []
  for await (const chunk of stream) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk))
  }
  return Buffer.concat(chunks)
}

function assertDirectory(path: string, label: string): void {
  try {
    if (statSync(path).isDirectory()) {
      return
    }
  } catch {
  }
  throw new Error(`Missing ${label}: ${path}`)
}
