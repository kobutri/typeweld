import { execFileSync } from "node:child_process"
import { readFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..")

type CargoDependency = {
  name: string
  req: string
}

type CargoPackage = {
  name: string
  version: string
  categories?: readonly string[]
  dependencies: readonly CargoDependency[]
  description?: string
  keywords?: readonly string[]
  license?: string
  readme?: string
  repository?: string
}

type CargoMetadata = {
  packages: readonly CargoPackage[]
}

type NpmManifest = {
  name?: string
  version?: string
  bin?: Record<string, string>
  dependencies?: Record<string, string>
  exports?: Record<string, unknown>
  files?: readonly string[]
  license?: string
  private?: boolean
  publishConfig?: {
    access?: string
  }
  repository?: {
    url?: string
  }
  types?: string
}

type PackageLock = {
  packages?: Record<string, {
    name?: string
    version?: string
  }>
}

type NpmReleasePackage = readonly [
  packageName: string,
  manifestPath: string,
  lockPath: string | null,
]

const releaseCrates: readonly string[] = [
  "typeweld-ir",
  "typeweld-build",
  "typeweld-core",
  "typeweld-axum",
  "typeweld-gen-effect-v4",
  "typeweld-macros",
  "typeweld-cli",
  "typeweld-ls",
]

const npmReleasePackages: readonly NpmReleasePackage[] = [
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

const npmPublicPackages = new Set<string>([
  "typeweld",
  "@typeweld/effect-runtime",
  "@typeweld/language-server",
])
const expectedNpmRepositoryUrl = "https://github.com/typeweld/typeweld.git"

const checkedInternalRustDependencies = new Set(releaseCrates)
checkedInternalRustDependencies.add("typeweld-test-fixtures")

function readJson<T>(path: string): T {
  return JSON.parse(readFileSync(resolve(repoRoot, path), "utf8")) as T
}

function fail(messages: readonly string[]): never {
  for (const message of messages) {
    console.error(message)
  }
  process.exit(1)
}

const errors: string[] = []
const cargoMetadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
    cwd: repoRoot,
    encoding: "utf8",
  }),
) as CargoMetadata

const cargoPackages = new Map(
  cargoMetadata.packages.map((cargoPackage) => [cargoPackage.name, cargoPackage]),
)

const expectedVersions: { name: string; version: string }[] = []

for (const crateName of releaseCrates) {
  const cargoPackage = cargoPackages.get(crateName)
  if (!cargoPackage) {
    errors.push(`missing Cargo package ${crateName}`)
    continue
  }
  expectedVersions.push({ name: crateName, version: cargoPackage.version })
}

const expectedVersion = expectedVersions[0]?.version

if (!expectedVersion) {
  fail(errors.length === 0 ? ["could not derive release version"] : errors)
}

const requiredCargoMetadata: readonly (keyof Pick<
  CargoPackage,
  "description" | "license" | "readme" | "repository"
>)[] = [
  "description",
  "license",
  "repository",
  "readme",
]

for (const { name, version } of expectedVersions) {
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

const npmRootManifest = readJson<NpmManifest>("npm/package.json")
if (npmRootManifest.private !== true) {
  errors.push("npm/package.json must stay private so the workspace root is never published")
}

for (const [packageName, manifestPath] of npmReleasePackages) {
  const manifest = readJson<NpmManifest>(manifestPath)
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
  if (
    npmPublicPackages.has(packageName) &&
    manifest.repository?.url !== expectedNpmRepositoryUrl
  ) {
    errors.push(
      `${packageName} repository.url must be ${expectedNpmRepositoryUrl} for npm trusted publishing`,
    )
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

const cliManifest = readJson<NpmManifest>("npm/cli/package.json")
if (cliManifest.bin?.typeweld !== "./dist/index.js") {
  errors.push("typeweld npm package must expose the typeweld CLI bin")
}

const effectRuntimeManifest = readJson<NpmManifest>("npm/effect-runtime/package.json")
if (effectRuntimeManifest.types !== "./src/index.ts") {
  errors.push("@typeweld/effect-runtime must expose ./src/index.ts as types")
}
if (!effectRuntimeManifest.exports?.["."] || !effectRuntimeManifest.exports?.["./compat"]) {
  errors.push("@typeweld/effect-runtime must export . and ./compat")
}

const languageServerManifest = readJson<NpmManifest>("npm/language-server-wrapper/package.json")
if (languageServerManifest.bin?.["typeweld-ls"] !== "./dist/index.js") {
  errors.push("@typeweld/language-server must expose the typeweld-ls CLI bin")
}

const vscodeManifest = readJson<NpmManifest>("npm/vscode-extension/package.json")
if (
  vscodeManifest.dependencies?.["@typeweld/language-server"] !== expectedVersion
) {
  errors.push(
    `typeweld-vscode depends on @typeweld/language-server ${vscodeManifest.dependencies?.["@typeweld/language-server"]}; expected ${expectedVersion}`,
  )
}

const packageLock = readJson<PackageLock>("npm/package-lock.json")
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
