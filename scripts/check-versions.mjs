#!/usr/bin/env node

import { execFileSync } from "node:child_process"
import { readFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..")

const releaseCrates = [
  "typeweld-ir",
  "typeweld-build",
  "typeweld-core",
  "typeweld-axum",
  "typeweld-gen-effect-v4",
  "typeweld-macros",
  "typeweld-cli",
  "typeweld-ls",
]

const npmReleasePackages = [
  ["typeweld", "npm/cli/package.json", "cli"],
  ["@typeweld/effect-runtime", "npm/effect-runtime/package.json", "effect-runtime"],
  [
    "@typeweld/language-server",
    "npm/language-server-wrapper/package.json",
    "language-server-wrapper",
  ],
  ["typeweld-vscode", "npm/vscode-extension/package.json", "vscode-extension"],
  [
    "@typeweld/typescript-plugin",
    "npm/vscode-extension/typescript-plugin/package.json",
    null,
  ],
]

const npmPublicPackages = new Set([
  "typeweld",
  "@typeweld/effect-runtime",
  "@typeweld/language-server",
])

const checkedInternalRustDependencies = new Set(releaseCrates)
checkedInternalRustDependencies.add("typeweld-test-fixtures")

function readJson(path) {
  return JSON.parse(readFileSync(resolve(repoRoot, path), "utf8"))
}

function fail(messages) {
  for (const message of messages) {
    console.error(message)
  }
  process.exit(1)
}

function versionSource(version, source) {
  return `${source}: ${version}`
}

const errors = []
const cargoMetadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
    cwd: repoRoot,
    encoding: "utf8",
  }),
)

const cargoPackages = new Map(
  cargoMetadata.packages.map((cargoPackage) => [cargoPackage.name, cargoPackage]),
)

const expectedVersions = []

for (const crateName of releaseCrates) {
  const cargoPackage = cargoPackages.get(crateName)
  if (!cargoPackage) {
    errors.push(`missing Cargo package ${crateName}`)
    continue
  }
  expectedVersions.push(versionSource(cargoPackage.version, crateName))
}

const expectedVersion = expectedVersions[0]?.split(": ")[1]

if (!expectedVersion) {
  fail(errors.length === 0 ? ["could not derive release version"] : errors)
}

const requiredCargoMetadata = [
  "description",
  "license",
  "repository",
  "readme",
]

for (const source of expectedVersions) {
  const [name, version] = source.split(": ")
  const cargoPackage = cargoPackages.get(name)
  if (version !== expectedVersion) {
    errors.push(
      `release crate ${name} uses version ${version}; expected ${expectedVersion}`,
    )
  }
  for (const field of requiredCargoMetadata) {
    if (!cargoPackage?.[field]) {
      errors.push(`release crate ${name} is missing Cargo metadata ${field}`)
    }
  }
  if (!cargoPackage?.keywords?.length) {
    errors.push(`release crate ${name} is missing Cargo metadata keywords`)
  }
  if (!cargoPackage?.categories?.length) {
    errors.push(`release crate ${name} is missing Cargo metadata categories`)
  }
}

for (const cargoPackage of cargoMetadata.packages) {
  if (!checkedInternalRustDependencies.has(cargoPackage.name)) {
    continue
  }

  for (const dependency of cargoPackage.dependencies) {
    if (
      dependency.name.startsWith("typeweld-") &&
      checkedInternalRustDependencies.has(dependency.name) &&
      dependency.req !== `^${expectedVersion}` &&
      dependency.req !== expectedVersion
    ) {
      errors.push(
        `${cargoPackage.name} depends on ${dependency.name} ${dependency.req}; expected ${expectedVersion}`,
      )
    }
  }
}

const npmRootManifest = readJson("npm/package.json")
if (npmRootManifest.private !== true) {
  errors.push("npm/package.json must stay private so the workspace root is never published")
}

for (const [packageName, manifestPath] of npmReleasePackages) {
  const manifest = readJson(manifestPath)
  if (manifest.name !== packageName) {
    errors.push(
      `${manifestPath} has package name ${manifest.name}; expected ${packageName}`,
    )
  }
  if (manifest.version !== expectedVersion) {
    errors.push(
      `${packageName} uses npm version ${manifest.version}; expected ${expectedVersion}`,
    )
  }
  if (!manifest.license) {
    errors.push(`${packageName} is missing npm license metadata`)
  }
  if (!manifest.repository) {
    errors.push(`${packageName} is missing npm repository metadata`)
  }
  if (!manifest.files?.length) {
    errors.push(`${packageName} is missing npm files metadata`)
  }
  if (
    npmPublicPackages.has(packageName) &&
    manifest.publishConfig?.access !== "public"
  ) {
    errors.push(`${packageName} must set publishConfig.access to public`)
  }
}

const cliManifest = readJson("npm/cli/package.json")
if (cliManifest.bin?.typeweld !== "./src/index.mjs") {
  errors.push("typeweld npm package must expose the typeweld CLI bin")
}

const effectRuntimeManifest = readJson("npm/effect-runtime/package.json")
if (effectRuntimeManifest.types !== "./src/index.ts") {
  errors.push("@typeweld/effect-runtime must expose ./src/index.ts as types")
}
if (!effectRuntimeManifest.exports?.["."] || !effectRuntimeManifest.exports?.["./compat"]) {
  errors.push("@typeweld/effect-runtime must export . and ./compat")
}

const languageServerManifest = readJson("npm/language-server-wrapper/package.json")
if (languageServerManifest.bin?.["typeweld-ls"] !== "./src/index.ts") {
  errors.push("@typeweld/language-server must expose the typeweld-ls CLI bin")
}

const vscodeManifest = readJson("npm/vscode-extension/package.json")
if (
  vscodeManifest.dependencies?.["@typeweld/language-server"] !== expectedVersion
) {
  errors.push(
    `typeweld-vscode depends on @typeweld/language-server ${vscodeManifest.dependencies?.["@typeweld/language-server"]}; expected ${expectedVersion}`,
  )
}

const packageLock = readJson("npm/package-lock.json")
for (const [packageName, , lockPath] of npmReleasePackages) {
  if (!lockPath) {
    continue
  }
  const lockedPackage = packageLock.packages?.[lockPath]
  if (!lockedPackage) {
    errors.push(`npm/package-lock.json is missing workspace entry ${lockPath}`)
    continue
  }
  if (lockedPackage.name !== packageName) {
    errors.push(
      `npm/package-lock.json entry ${lockPath} has package name ${lockedPackage.name}; expected ${packageName}`,
    )
  }
  if (lockedPackage.version !== expectedVersion) {
    errors.push(
      `npm/package-lock.json entry ${lockPath} uses version ${lockedPackage.version}; expected ${expectedVersion}`,
    )
  }
}

const githubRefName = process.env.GITHUB_REF_NAME
if (githubRefName?.startsWith("v")) {
  const tagVersion = githubRefName.slice(1)
  if (tagVersion !== expectedVersion) {
    errors.push(
      `GitHub tag ${githubRefName} implies version ${tagVersion}; expected ${expectedVersion}`,
    )
  }
}

if (errors.length > 0) {
  fail(errors)
}

console.log(`Typeweld release versions match: ${expectedVersion}`)
