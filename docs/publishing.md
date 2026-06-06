# Publishing Typeweld

This project has three release surfaces:

- crates.io crates: `typeweld-ir`, `typeweld-build`, `typeweld-core`,
  `typeweld-axum`, `typeweld-gen-effect-v4`, `typeweld-macros`,
  `typeweld-cli`, and `typeweld-ls`.
- npm packages: `typeweld`, `@typeweld/effect-runtime`, and
  `@typeweld/language-server`.
- GitHub release assets: VSIX files plus standalone `typeweld` CLI archives.

## Recommendation

Do not keep a manually-triggerable package publishing workflow in this
repository. A compromised local machine or browser session should not be able to
publish packages by opening GitHub Actions and pressing a dispatch button.

The current repository setup intentionally has no npm/crates.io publish workflow.
Registry publishing should stay deferred until a release process is designed
around protected tags or published GitHub Releases, protected environments,
short-lived credentials, and reviewable release PRs.

The repository now has:

- `.github/workflows/release-vscode-extension.yml` for GitHub release assets,
  triggered only by a published GitHub Release.
- `npm run prepare:binary --workspace typeweld` to place a native CLI binary in
  `npm/cli/bin/<platform>/`.
- `npm run prepare:binary --workspace @typeweld/language-server` to place a
  native `typeweld-ls` binary in `npm/language-server-wrapper/bin/<platform>/`.

## Options

Option A, recommended now: no registry publish workflow.

- Version bumps and changelog stay reviewable.
- Package dry-runs happen locally and in ordinary CI.
- Actual registry publication requires a deliberate, separate operator action
  from a hardened environment.
- There is no `workflow_dispatch` path that can publish packages.

Option B, later: protected release-event publishing.

- Trigger only from protected tags or `release.published`, never
  `workflow_dispatch`.
- Use GitHub environments with required reviewers.
- Use npm/crates.io trusted publishing so CI gets short-lived OIDC credentials
  instead of long-lived registry tokens.

Option C, later: release-plz or cargo-release for Rust version bumps.

- Good once the release cadence stabilizes.
- Still keep npm package versions and native binary packaging in the same release
  checklist.
- Adds another moving part before the first public release.

## One-Time Registry Setup

Create or join the npm org/scope that will own `@typeweld/*`, then reserve these
npm packages:

- `typeweld`
- `@typeweld/effect-runtime`
- `@typeweld/language-server`

Do not configure npm or crates.io trusted publishers until the corresponding
release-event-only workflow exists. When that workflow is added, configure npm
trusted publishing for each package:

- Provider: GitHub Actions
- Organization/user: `typeweld`
- Repository: `typeweld`
- Workflow filename: the future release-event-only workflow
- Environment: `release`
- Allowed action: `npm publish`

On crates.io, create or reserve each crate name, then configure trusted
publishing for the same repository/workflow/environment for each publishable
crate. If crates.io still requires the first crate version to be published
manually, publish `0.0.1` locally once, then enable trusted publishing only after
the release-event workflow exists.

Create a GitHub environment named `release` and require reviewer approval before
adding any registry publishing workflow.

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

3. Create and publish a GitHub Release for tag `vX.Y.Z`.

   The release workflow uploads VSIX and standalone CLI assets. Release-built CLI
   binaries are compiled with `TYPEWELD_TEMPLATE_SOURCE=github`, so `typeweld new`
   generated from those binaries points at GitHub release assets and git-tagged
   Rust dependencies.

4. Publish registry packages outside repository Actions.

   Until there is a protected release-event-only workflow, publish npm/crates.io
   packages only from a hardened environment with short-lived authentication.

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
