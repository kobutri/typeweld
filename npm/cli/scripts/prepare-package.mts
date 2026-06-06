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
const platformDir =
  parseOption(process.argv.slice(2), "--platform-dir") ??
  `${process.platform}-${process.arch}`
const outDir = join(packageRoot, "bin", platformDir)
const outFile = join(outDir, binaryName)

const explicitBinary =
  parseOption(process.argv.slice(2), "--typeweld-binary") ?? process.env.TYPEWELD_BINARY
const candidates = [
  explicitBinary,
  join(repoRoot, "target", "release", binaryName),
  join(repoRoot, "target", "debug", binaryName),
].filter((value): value is string => value !== undefined && value.trim() !== "")

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

function parseOption(args: readonly string[], name: string): string | undefined {
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index]
    if (arg === undefined) {
      continue
    }
    if (arg === name) {
      return args[index + 1]
    }
    if (arg.startsWith(`${name}=`)) {
      return arg.slice(name.length + 1)
    }
  }
  return undefined
}

function isExecutable(path: string | undefined): boolean {
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
