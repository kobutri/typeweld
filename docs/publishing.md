# Publishing Typeweld

Typeweld is released as one versioned product across two package ecosystems:

- Rust crates on crates.io: `typeweld-ir`, `typeweld-build`, `typeweld-core`,
  `typeweld-axum`, `typeweld-gen-effect-v4`, `typeweld-macros`,
  `typeweld-cli`, and `typeweld-ls`.
- npm packages: `typeweld`, `@typeweld/effect-runtime`, and
  `@typeweld/language-server`.
- VS Code extension marketplaces: Visual Studio Marketplace and Open VSX.
- GitHub release assets: VSIX files plus standalone `typeweld` CLI archives.

All publishable Typeweld crates, public npm packages, and VSIX/package-lock
release entries use the same SemVer version. The root npm workspace stays
`private: true`; only leaf packages are publishable.

## Guardrails

Do not keep a manually-triggerable package publishing workflow in this
repository. A compromised local machine or browser session should not be able to
publish packages by opening GitHub Actions and pressing a dispatch button.

Publishing is driven by GitHub Release publication. Draft release creation does
not publish anything; publishing a non-prerelease `vX.Y.Z` GitHub Release starts
the release workflows.

Registry publishing uses the protected `release` environment and short-lived
OIDC credentials where the registries support them. npm and crates.io publish
jobs do not use repository-wide package tokens. The npm packages are packed
before the OIDC-enabled publish job, then `npm publish` uploads those tarballs
with lifecycle scripts disabled. Rust crates are published in dependency order
with `cargo publish --no-verify`; package listing and normal CI verify the
contents before the protected publish job gets an OIDC token.

VS Code extension marketplace publishing is also part of the GitHub Release
path. The marketplace job uses the protected `release` environment, installs
pinned publisher CLIs with lifecycle scripts disabled, and publishes the
already-built VSIX artifacts instead of rebuilding while marketplace credentials
are present.

The repository has:

- `.github/workflows/release-vscode-extension.yml` for GitHub release assets,
  Visual Studio Marketplace publishing, and Open VSX publishing, triggered only
  by a published GitHub Release.
- `.github/workflows/release-packages.yml` for crates.io and npm publishing,
  triggered only by a published GitHub Release.
- `npm run check:versions`, which verifies release crate versions, npm package
  versions, npm lockfile workspace versions, VS Code extension local plugin wiring,
  public npm repository URLs, internal Rust dependency pins, and `GITHUB_REF_NAME`
  tags.
- `npm run prepare:binary --workspace typeweld` to place a native CLI binary in
  `npm/cli/bin/<platform>/`.
- `npm run prepare:binary --workspace @typeweld/language-server` to place a
  native `typeweld-ls` binary in `npm/language-server-wrapper/bin/<platform>/`.

## Release Workflow

`.github/workflows/release-packages.yml` has four stages:

- `check-release`: checks the non-prerelease `vX.Y.Z` tag, release-train
  versions, Cargo package contents, and npm dry-run packing without package
  credentials.
- `build-npm-native-binaries`: builds `typeweld` and `typeweld-ls` for
  `linux-x64`, `darwin-x64`, `darwin-arm64`, and `win32-x64`. npm-packaged
  `typeweld` binaries use `TYPEWELD_TEMPLATE_SOURCE=registry`, so
  `npx typeweld new` generates npm/crates.io dependencies.
- `pack-npm`: downloads those native binaries, builds the TypeScript launchers,
  and produces npm tarballs without OIDC publish credentials.
- `publish-registries`: waits for the protected `release` environment approval,
  gets short-lived OIDC credentials, publishes crates first, then publishes the
  npm tarballs. Already-published package versions are skipped so a failed
  release rerun can continue after a partial registry publish.

`.github/workflows/release-vscode-extension.yml` separately builds VSIX files and
standalone CLI archives. Those standalone CLI archives still use
`TYPEWELD_TEMPLATE_SOURCE=github`, so projects generated from downloaded GitHub
release binaries point at GitHub release assets and git-tagged Rust dependencies.

No workflow has `workflow_dispatch` for publishing.

## One-Time Registry Setup

Create or join the npm org/scope that will own `@typeweld/*`, then reserve these
npm packages:

- `typeweld`
- `@typeweld/effect-runtime`
- `@typeweld/language-server`

Plan to claim these crates on crates.io during the first publish:

- `typeweld-ir`
- `typeweld-build`
- `typeweld-core`
- `typeweld-axum`
- `typeweld-gen-effect-v4`
- `typeweld-macros`
- `typeweld-cli`
- `typeweld-ls`

The Rust manifests should keep crates.io metadata populated through
`workspace.package`: description, license, repository, readme, keywords, and
categories. npm leaf packages should keep package metadata, `files`, `bin` or
`exports`, and `publishConfig.access` where applicable.

Set up the VS Code extension registries:

- Visual Studio Marketplace: create the `kobutri` publisher and authorize the
  release identity or token for extension publishing.
- Open VSX: create the `kobutri` namespace, sign the Eclipse Publisher
  Agreement, and generate an access token for CI.
- GitHub: store `VSCE_PAT` and `OVSX_PAT` as `release` environment secrets, not
  repository-wide secrets, and require reviewer approval for that environment.

Create a GitHub environment named `release` and require reviewer approval. The
registry and marketplace publish jobs all use this environment.

Configure npm trusted publishing for each public npm package:

- Provider: GitHub Actions
- Organization/user: `kobutri`
- Repository: `typeweld`
- Workflow filename: `release-packages.yml`
- Environment: `release`
- Allowed action: `npm publish`

The npm CLI also supports:

```sh
npm trust github typeweld --repo kobutri/typeweld --file release-packages.yml --env release --yes
npm trust github @typeweld/effect-runtime --repo kobutri/typeweld --file release-packages.yml --env release --yes
npm trust github @typeweld/language-server --repo kobutri/typeweld --file release-packages.yml --env release --yes
```

On crates.io, configure trusted publishing for each publishable crate with:

- GitHub organization/user: `kobutri`
- Repository: `typeweld`
- Workflow filename: `release-packages.yml`
- Environment: `release`

npm and crates.io both require the package/crate to already exist before adding
these trusted publisher settings. For the first ever version, bootstrap the
package names from a hardened environment or a temporary, protected release
environment token, then immediately configure trusted publishing and remove or
disable long-lived publish tokens. After that bootstrap, normal publishing is
done by creating a GitHub Release.

## Release Walkthrough

1. Prepare a release PR.

   Update all package versions together:

   - `Cargo.toml` workspace version.
   - Internal `typeweld-*` dependency versions in crate manifests.
   - `npm/cli/package.json`, `npm/effect-runtime/package.json`,
     `npm/language-server-wrapper/package.json`, `npm/vscode-extension/package.json`,
     and `npm/vscode-extension/typescript-plugin/package.json`.
   - `npm/package-lock.json` via `npm install` from `npm/`.

2. Run local preflight.

   ```sh
   cargo fmt --all --check
   cargo clippy --workspace --all-targets
   cargo test --workspace
   npm --prefix npm ci
   npm --prefix npm run check:versions
   npm --prefix npm test
   cargo publish -p typeweld-ir --dry-run
   cargo package -p typeweld-ir --list
   npm --prefix npm pack --dry-run --workspace typeweld
   npm --prefix npm pack --dry-run --workspace @typeweld/effect-runtime
   npm --prefix npm pack --dry-run --workspace @typeweld/language-server
   npm --prefix npm/vscode-extension run package:vsix -- --target linux-x64 --out out/typeweld-vscode-preflight-linux-x64.vsix
   ```

   `cargo publish --dry-run` works for leaf crates before the first publish. For
   crates that depend on unpublished Typeweld crates, Cargo still checks the
   crates.io index for the internal dependency versions, so the full dry-run is
   only meaningful after the lower-level crates have been published once.

3. Create and publish a GitHub Release for tag `vX.Y.Z`.

   Publishing the release starts both release workflows:

   - `.github/workflows/release-packages.yml` publishes crates.io and npm
     packages from the protected `release` environment.
   - `.github/workflows/release-vscode-extension.yml` uploads VSIX and
     standalone CLI assets, then publishes the VSIX artifacts to Visual Studio
     Marketplace and Open VSX from the protected `release` environment.

4. Approve the `release` environment deployment.

   The registry publish job publishes Rust crates in dependency order:

   ```text
   typeweld-ir
   typeweld-build
   typeweld-core
   typeweld-axum
   typeweld-gen-effect-v4
   typeweld-macros
   typeweld-cli
   typeweld-ls
   ```

   It then publishes npm packages:

   ```text
   @typeweld/effect-runtime
   @typeweld/language-server
   typeweld
   ```

5. Verify the published artifacts.

   ```sh
   npm view typeweld version
   npm view @typeweld/effect-runtime version
   npm view @typeweld/language-server version
   cargo search typeweld-cli --limit 1
   ```

   Also install the VSIX for one platform from the GitHub Release assets and run:

   ```sh
   npx typeweld new smoke --yes
   ```

## Source Notes

- npm trusted publishing uses OIDC and short-lived workflow credentials, and npm
  recommends it over long-lived tokens.
- npm trusted publishing from GitHub Actions requires `id-token: write` and
  package-level trusted publisher configuration.
- `npm trust` requires npm 11.10 or newer, write access to an existing package,
  and a package that already exists on the npm registry.
- crates.io trusted publishing lets a GitHub Actions workflow publish without a
  long-lived crates.io token.
- crates.io trusted publisher configuration currently requires the crate to have
  an initial published version before OIDC publishing can take over.
- Cargo recommends `cargo publish --dry-run` and `cargo package --list` before
  publishing.
- VS Code supports platform-specific VSIX packages and `vsce publish
  --packagePath` for publishing prebuilt VSIX files.
- VS Code's current security guidance prefers Microsoft Entra ID-based
  publishing over long-lived Marketplace PATs where the release infrastructure
  supports it.
- Open VSX publishes prebuilt VSIX files with `ovsx publish <file>` or
  `ovsx publish --packagePath`.

References:

- <https://docs.npmjs.com/trusted-publishers/>
- <https://docs.npmjs.com/cli/v11/commands/npm-trust/>
- <https://forge.rust-lang.org/infra/docs/trusted-publishing.html>
- <https://rust-lang.github.io/rfcs/3691-trusted-publishing-cratesio.html>
- <https://doc.rust-lang.org/cargo/reference/publishing.html>
- <https://docs.github.com/en/actions/concepts/security/openid-connect>
- <https://code.visualstudio.com/api/working-with-extensions/publishing-extension>
- <https://github.com/eclipse-openvsx/openvsx/wiki/Publishing-Extensions>
