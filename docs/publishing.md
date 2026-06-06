# Publishing Typeweld

This project has three release surfaces:

- crates.io crates: `typeweld-ir`, `typeweld-build`, `typeweld-core`,
  `typeweld-axum`, `typeweld-gen-effect-v4`, `typeweld-macros`,
  `typeweld-cli`, and `typeweld-ls`.
- npm packages: `typeweld`, `@typeweld/effect-runtime`, and
  `@typeweld/language-server`.
- GitHub release assets: VSIX files plus standalone `typeweld` CLI archives.

## Recommendation

Use GitHub Actions trusted publishing for crates.io and npm, with a manual
workflow dispatch for the first few releases. That gives you short-lived OIDC
credentials instead of long-lived registry tokens, while keeping a human approval
step before anything is published.

The repository now has:

- `.github/workflows/publish.yml` for crates.io and npm dry runs or publishes.
- `.github/workflows/release-vscode-extension.yml` for GitHub release assets.
- `npm run prepare:binary --workspace typeweld` to place a native CLI binary in
  `npm/cli/bin/<platform>/`.
- `npm run prepare:binary --workspace @typeweld/language-server` to place a
  native `typeweld-ls` binary in `npm/language-server-wrapper/bin/<platform>/`.

## Options

Option A, recommended now: manual release PR plus manual publish workflow.

- Version bumps and changelog stay reviewable.
- The publish workflow defaults to `dry_run: true`.
- npm and crates.io can use trusted publishing once configured.
- GitHub release assets are tied to an actual GitHub Release.

Option B, later: release-plz or cargo-release for Rust version bumps.

- Good once the release cadence stabilizes.
- Still keep npm package versions and native binary packaging in the same release
  checklist.
- Adds another moving part before the first public release, so this is not the
  first setup.

Option C, not recommended: local machine publishing.

- Fine for the first name reservation if crates.io requires it.
- Worse long-term because it depends on local auth tokens and is harder to audit.

## One-Time Registry Setup

Create or join the npm org/scope that will own `@typeweld/*`, then reserve these
npm packages:

- `typeweld`
- `@typeweld/effect-runtime`
- `@typeweld/language-server`

On npmjs.com, configure a trusted publisher for each package:

- Provider: GitHub Actions
- Organization/user: `typeweld`
- Repository: `typeweld`
- Workflow filename: `publish.yml`
- Environment: `release`
- Allowed action: `npm publish`

On crates.io, create or reserve each crate name, then configure trusted
publishing for the same repository/workflow/environment for each publishable
crate. If crates.io still requires the first crate version to be published
manually, publish `0.0.1` locally once, then enable trusted publishing for later
versions.

Create a GitHub environment named `release` and require reviewer approval. Both
the npm and crates.io publishing jobs use that environment.

## Release Walkthrough

1. Prepare a release PR.

   Update all package versions together:

   - `Cargo.toml` workspace version.
   - Internal `typeweld-*` dependency versions in crate manifests.
   - `npm/cli/package.json`, `npm/effect-runtime/package.json`,
     `npm/language-server-wrapper/package.json`, `npm/vscode-extension/package.json`,
     and `npm/vscode-extension/typescript-plugin/package.json`.
   - `npm/package-lock.json` via `npm install` from `npm/`.

2. Run local checks.

   ```sh
   cargo test --workspace
   npm --prefix npm test
   cargo publish -p typeweld-ir --dry-run
   npm --prefix npm pack --dry-run --workspace typeweld
   ```

   `cargo publish --dry-run` works for leaf crates before the first publish. For
   crates that depend on unpublished Typeweld crates, Cargo still checks the
   crates.io index for the internal dependency versions, so the full dry-run is
   only meaningful after the lower-level crates have been published once.

3. Run the GitHub publish workflow in dry-run mode.

   Use Actions -> Publish Packages:

   - `version`: the release version, for example `0.1.0`
   - `dry_run`: `true`
   - `publish_crates`: `true`
   - `publish_npm`: `true`

4. Create and publish a GitHub Release for tag `vX.Y.Z`.

   The release workflow uploads VSIX and standalone CLI assets. Release-built CLI
   binaries are compiled with `TYPEWELD_TEMPLATE_SOURCE=github`, so `typeweld new`
   generated from those binaries points at GitHub release assets and git-tagged
   Rust dependencies.

5. Publish packages.

   Re-run Publish Packages with:

   - `dry_run`: `false`
   - `publish_crates`: `true`
   - `publish_npm`: `true`

   npm-published CLI binaries are compiled with `TYPEWELD_TEMPLATE_SOURCE=registry`,
   so `npx typeweld new` generates npm and crates.io dependencies.

## Source Notes

- npm trusted publishing uses OIDC and requires npm CLI 11.5.1+ with Node
  22.14.0+; npm publishes provenance automatically when trusted publishing is
  used from GitHub Actions.
- crates.io recommends `cargo publish --dry-run` before upload and documents the
  normal `cargo publish` flow.
- GitHub Actions OIDC avoids long-lived registry secrets by issuing short-lived,
  workflow-scoped tokens.

References:

- <https://docs.npmjs.com/trusted-publishers/>
- <https://docs.npmjs.com/generating-provenance-statements/>
- <https://doc.rust-lang.org/cargo/reference/publishing.html>
- <https://crates.io/docs/trusted-publishing>
- <https://docs.github.com/en/actions/concepts/security/openid-connect>
