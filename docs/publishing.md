# Publishing Typeweld

Typeweld is released as one versioned product across two package ecosystems:

- Rust crates on crates.io: `typeweld-ir`, `typeweld-build`, `typeweld-core`,
  `typeweld-axum`, `typeweld-gen-effect-v4`, `typeweld-macros`,
  `typeweld-cli`, and `typeweld-ls`.
- npm packages: `typeweld`, `@typeweld/effect-runtime`, and
  `@typeweld/language-server`.
- GitHub release assets: VSIX files plus standalone `typeweld` CLI archives.

All publishable Typeweld crates, public npm packages, and VSIX/package-lock
release entries use the same SemVer version. The root npm workspace stays
`private: true`; only leaf packages are publishable.

## Guardrails

Do not keep a manually-triggerable package publishing workflow in this
repository. A compromised local machine or browser session should not be able to
publish packages by opening GitHub Actions and pressing a dispatch button.

The current repository intentionally has no npm/crates.io publish workflow.
Registry publishing stays outside repository Actions until a release process is
built around protected tags or published GitHub Releases, protected
environments, short-lived credentials, and reviewable release PRs.

The repository has:

- `.github/workflows/release-vscode-extension.yml` for GitHub release assets,
  triggered only by a published GitHub Release.
- `npm run check:versions`, which verifies release crate versions, npm package
  versions, npm lockfile workspace versions, VS Code extension dependency pins,
  internal Rust dependency pins, and `GITHUB_REF_NAME` tags.
- `npm run prepare:binary --workspace typeweld` to place a native CLI binary in
  `npm/cli/bin/<platform>/`.
- `npm run prepare:binary --workspace @typeweld/language-server` to place a
  native `typeweld-ls` binary in `npm/language-server-wrapper/bin/<platform>/`.

## Options

Option A, recommended now: no registry publish workflow.

- Version bumps and changelog stay reviewable.
- Package dry-runs happen locally and in ordinary CI.
- The first registry publish is a deliberate operator step from a hardened
  environment with fresh authentication.
- There is no manual GitHub Actions dispatch path that can publish packages.

Option B, later: protected tag publishing with trusted publishers.

- Trigger only from protected tags such as `v*` or from `release.published`,
  never from manual dispatch.
- Use GitHub environments with required reviewers.
- Use npm/crates.io trusted publishing so CI gets short-lived OIDC credentials
  instead of long-lived registry tokens.
- Publish crates first, then npm packages. If npm fails after crates are
  published, fix forward with a new patch version because registry versions are
  immutable.

Option C, later: release-plz or cargo-release for Rust version bumps.

- Good once the release cadence stabilizes.
- Still keep npm package versions, package-lock versions, and native binary
  packaging in the same release checklist.
- Adds another moving part before the first public release.

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

Do not configure npm or crates.io trusted publishers until the corresponding
tag/release-event-only workflow exists. When that workflow is added, configure
npm trusted publishing for each package:

- Provider: GitHub Actions
- Organization/user: `typeweld`
- Repository: `typeweld`
- Workflow filename: the future tag/release-event-only workflow
- Environment: `release`
- Allowed action: `npm publish`

On crates.io, configure trusted publishing for the same repository, workflow,
and protected `release` environment for each publishable crate. If a registry
requires the package or crate to exist before configuring trusted publishing,
publish the first version once from a hardened environment, then enable trusted
publishing and disallow long-lived publish tokens where the registry supports
that.

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

   Until there is a protected tag/release-event-only workflow, publish
   npm/crates.io packages only from a hardened environment with short-lived
   authentication. Publish Rust crates in dependency order, then publish npm
   packages.

## Future Trusted Publish Workflow

The future registry workflow should be added only after the release environment,
tag protection, package ownership, and trusted publisher settings are ready.

Sketch:

```yaml
on:
  push:
    tags:
      - "v*"

permissions:
  contents: read

jobs:
  publish:
    environment: release
    permissions:
      contents: read
      id-token: write
    steps:
      - uses: actions/checkout@v4
      - run: npm --prefix npm ci
      - run: npm --prefix npm run check:versions
      - run: cargo publish -p typeweld-ir
      - run: npm publish --workspace typeweld
```

The real workflow needs all crates in dependency order, all public npm packages,
binary packaging steps, and no registry tokens stored in repository secrets.

## Source Notes

- npm trusted publishing uses OIDC and short-lived workflow credentials, and npm
  recommends it over long-lived tokens.
- npm trusted publishing from GitHub Actions requires `id-token: write` and
  package-level trusted publisher configuration.
- crates.io trusted publishing lets a GitHub Actions workflow publish without a
  long-lived crates.io token.
- Cargo recommends `cargo publish --dry-run` and `cargo package --list` before
  publishing.

References:

- <https://docs.npmjs.com/trusted-publishers/>
- <https://forge.rust-lang.org/infra/docs/trusted-publishing.html>
- <https://doc.rust-lang.org/cargo/reference/publishing.html>
- <https://docs.github.com/en/actions/concepts/security/openid-connect>
