#!/usr/bin/env node

import {
  accessSync,
  chmodSync,
  constants,
  copyFileSync,
  existsSync,
  mkdirSync,
  statSync,
} from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const scriptDir = dirname(fileURLToPath(import.meta.url))
const packageRoot = resolve(scriptDir, "..")
const repoRoot = resolve(packageRoot, "..", "..")
const binaryName = process.platform === "win32" ? "typeweld.exe" : "typeweld"
const outDir = join(packageRoot, "bin", `${process.platform}-${process.arch}`)
const outFile = join(outDir, binaryName)

const explicitBinary = parseBinaryArg(process.argv.slice(2)) ?? process.env.TYPEWELD_BINARY
const candidates = [
  explicitBinary,
  join(repoRoot, "target", "release", binaryName),
  join(repoRoot, "target", "debug", binaryName),
].filter((value) => value !== undefined && value.trim() !== "")

const source = candidates.find((candidate) => isExecutable(candidate))

if (source === undefined) {
  console.error(
    [
      "could not find a typeweld binary to package.",
      "",
      "Build one first:",
      "  cargo build -p typeweld-cli --bin typeweld --release",
      "",
      "Or pass it explicitly:",
      "  npm run prepare:binary --workspace typeweld -- --typeweld-binary /path/to/typeweld",
      "",
      "Checked:",
      ...candidates.map((candidate) => `  - ${candidate}`),
    ].join("\n"),
  )
  process.exit(1)
}

mkdirSync(outDir, { recursive: true })
copyFileSync(source, outFile)
chmodSync(outFile, 0o755)
console.log(`packaged ${source} -> ${outFile}`)

function parseBinaryArg(args) {
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index]
    if (arg === "--typeweld-binary") {
      return args[index + 1]
    }
    if (arg.startsWith("--typeweld-binary=")) {
      return arg.slice("--typeweld-binary=".length)
    }
  }
  return undefined
}

function isExecutable(path) {
  if (path === undefined || !existsSync(path)) {
    return false
  }
  try {
    if (!statSync(path).isFile()) {
      return false
    }
    accessSync(path, process.platform === "win32" ? constants.F_OK : constants.X_OK)
    return true
  } catch {
    return false
  }
}
