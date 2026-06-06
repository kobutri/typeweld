# Release Process

The release workflow packages platform-specific VSIX files and CLI binaries,
then attaches them to the GitHub Release after the release is published.

## What CI Builds

Regular CI runs the existing Rust and npm test suites, then does a Linux VSIX
smoke package. The smoke package builds `typeweld-ls` and the `typeweld` CLI in release
mode, copies both binaries into the extension under `bin/linux-x64/`, copies the
npm launcher into `server/index.js`, and runs `vsce package`.

## Release Outputs

Publishing a GitHub Release starts both the package registry workflow documented
in `docs/publishing.md` and `.github/workflows/release-vscode-extension.yml`.
The VS Code workflow builds these VSIX assets:

- `typeweld-<version>-linux-x64.vsix`
- `typeweld-<version>-darwin-x64.vsix`
- `typeweld-<version>-darwin-arm64.vsix`
- `typeweld-<version>-win32-x64.vsix`

Each VSIX includes the compiled extension bundle plus the matching `typeweld-ls` and
`typeweld` binaries. The extension manifest version is stamped from the release tag
during the workflow, so use tags like `v0.1.0`.

The same workflow also builds and uploads the `typeweld` CLI:

- `typeweld-<version>-linux-x64.tar.gz`
- `typeweld-<version>-darwin-x64.tar.gz`
- `typeweld-<version>-darwin-arm64.tar.gz`
- `typeweld-<version>-win32-x64.zip`

After the GitHub Release assets are uploaded, the same workflow enters the
protected `release` environment and publishes the four platform-specific VSIX
files to:

- Visual Studio Marketplace
- Open VSX Registry

The marketplace job downloads the VSIX artifacts built by the matrix job and
publishes those exact files. It does not rebuild the extension while registry
credentials are available.

## Marketplace Setup

Before the first marketplace release:

1. Create the `typeweld` publisher in Visual Studio Marketplace.
2. Create the `typeweld` namespace in Open VSX and sign the Eclipse Publisher
   Agreement.
3. Add release environment secrets:

   - `VSCE_PAT`: Visual Studio Marketplace token with Marketplace Manage scope.
   - `OVSX_PAT`: Open VSX access token for the `typeweld` namespace.

Microsoft recommends Entra ID-based publishing over long-lived Marketplace PATs
where the release infrastructure supports it. If Typeweld later moves extension
publishing to that model, replace `VSCE_PAT` with `vsce publish
--azure-credential` and keep the same release-event-only trigger and protected
environment.

The marketplace publish job uses `--skip-duplicate`, so rerunning a failed
release job can continue after a partially successful publish.

## Steps

1. Land the release commit on `main`.
2. Run the release preflight from `docs/publishing.md`.
3. Create a protected semver tag such as `v0.1.0`.
4. Create a GitHub Release for that tag.
5. Publish the release. Draft creation alone does not upload assets, publish
   package registries, or publish marketplaces.
6. Approve the `release` environment deployment when the marketplace publish job
   requests it.
7. Wait for the `Release Artifacts` workflow to finish.
8. Download the VSIX for your platform from the release assets and test install:

```sh
code --install-extension typeweld-0.1.0-darwin-arm64.vsix
```

If the release workflow needs to be rerun after fixing workflow-only issues,
delete the failed GitHub Release and recreate it from the same protected tag.
