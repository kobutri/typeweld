import { execFileSync } from "node:child_process"
import { readFileSync, writeFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const targetVersion = process.argv[2]

if (!targetVersion || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(targetVersion)) {
  console.error("usage: npm run bump:version -- <version>")
  process.exit(1)
}

const npmManifests = [
  "npm/cli/package.json",
  "npm/effect-runtime/package.json",
  "npm/language-server-wrapper/package.json",
  "npm/semantic-usage-scanner/package.json",
  "npm/vscode-extension/package.json",
  "npm/vscode-extension/typescript-plugin/package.json",
] as const

const cargoManifestsWithInternalDeps = [
  "crates/typeweld-axum/Cargo.toml",
  "crates/typeweld-cli/Cargo.toml",
  "crates/typeweld-core/Cargo.toml",
  "crates/typeweld-gen-effect-v4/Cargo.toml",
  "crates/typeweld-ls/Cargo.toml",
  "crates/typeweld-macros/Cargo.toml",
  "crates/typeweld-test-fixtures/Cargo.toml",
] as const

function read(path: string): string {
  return readFileSync(resolve(repoRoot, path), "utf8")
}

function write(path: string, contents: string): void {
  writeFileSync(resolve(repoRoot, path), contents)
}

function readJson(path: string): Record<string, unknown> {
  return JSON.parse(read(path)) as Record<string, unknown>
}

function writeJson(path: string, data: unknown): void {
  write(path, `${JSON.stringify(data, null, 2)}\n`)
}

function updateCargoWorkspaceVersion(): string {
  const path = "Cargo.toml"
  const cargoToml = read(path)
  const currentVersion = cargoToml.match(
    /^\[workspace\.package\][\s\S]*?^version = "([^"]+)"/m,
  )?.[1]

  if (!currentVersion) {
    console.error("could not find [workspace.package] version in Cargo.toml")
    process.exit(1)
  }

  write(
    path,
    cargoToml.replace(
      /^(\[workspace\.package\][\s\S]*?^version = ")[^"]+(")/m,
      `$1${targetVersion}$2`,
    ),
  )

  return currentVersion
}

function updateInternalCargoDependencyVersions(): void {
  for (const path of cargoManifestsWithInternalDeps) {
    const updated = read(path).replace(
      /^(typeweld-[\w-]+\s*=\s*\{\s*version\s*=\s*")[^"]+("\s*,\s*path\s*=)/gm,
      `$1${targetVersion}$2`,
    )
    write(path, updated)
  }
}

function updateNpmManifests(): void {
  for (const path of npmManifests) {
    const manifest = readJson(path)
    manifest.version = targetVersion

    if (path === "npm/vscode-extension/package.json") {
      const dependencies = manifest.dependencies as Record<string, string> | undefined
      if (dependencies?.["@typeweld/language-server"]) {
        dependencies["@typeweld/language-server"] = targetVersion
      }
    }

    writeJson(path, manifest)
  }
}

const currentVersion = updateCargoWorkspaceVersion()
updateInternalCargoDependencyVersions()
updateNpmManifests()

execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
  cwd: repoRoot,
  stdio: "ignore",
})
execFileSync("npm", ["install", "--package-lock-only"], {
  cwd: resolve(repoRoot, "npm"),
  stdio: "inherit",
})
execFileSync("npm", ["run", "check:versions"], {
  cwd: resolve(repoRoot, "npm"),
  stdio: "inherit",
})

console.log(`Bumped Typeweld from ${currentVersion} to ${targetVersion}`)
