# Release Process

The release workflow packages platform-specific VSIX files and CLI binaries,
then attaches them to the GitHub Release after the release is published.

## What CI Builds

Regular CI runs the existing Rust and npm test suites, then does a Linux VSIX
smoke package. The smoke package builds `api-ls` in release mode, copies the
binary into the extension under `bin/linux-x64/api-ls`, copies the npm launcher
into `server/index.js`, and runs `vsce package`.

## Release Assets

Publishing a GitHub Release starts `.github/workflows/release-vscode-extension.yml`.
The workflow builds these VSIX assets:

- `rust-ts-integration-<version>-linux-x64.vsix`
- `rust-ts-integration-<version>-darwin-x64.vsix`
- `rust-ts-integration-<version>-darwin-arm64.vsix`
- `rust-ts-integration-<version>-win32-x64.vsix`

Each VSIX includes the compiled extension bundle plus the matching `api-ls`
binary. The extension manifest version is stamped from the release tag during
the workflow, so use tags like `v0.1.0`.

The same workflow also builds and uploads the `api` CLI:

- `rust-ts-integration-api-<version>-linux-x64.tar.gz`
- `rust-ts-integration-api-<version>-darwin-x64.tar.gz`
- `rust-ts-integration-api-<version>-darwin-arm64.tar.gz`
- `rust-ts-integration-api-<version>-win32-x64.zip`

## Steps

1. Land the release commit on `main`.
2. Run `npm test` locally if you want a final preflight.
3. Create a GitHub Release for a semver tag such as `v0.1.0`.
4. Publish the release. Draft creation alone does not upload assets.
5. Wait for the `Release Artifacts` workflow to finish.
6. Download the VSIX for your platform from the release assets and test install:

```sh
code --install-extension rust-ts-integration-0.1.0-darwin-arm64.vsix
```

If the release workflow needs to be rerun after fixing workflow-only issues,
run it manually with `workflow_dispatch` and pass the same tag name.
