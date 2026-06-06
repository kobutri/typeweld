# Current Limitations

This repository is still an MVP implementation.

- Contract collection is metadata-first and does not yet discover every Rust
  API shape automatically from a full workspace.
- The TypeScript usage scanner is intentionally conservative and line-oriented.
  It recognizes common strong Effect patterns, but it is not a full TypeScript
  compiler pass.
- Generated Effect code targets the current Effect v4 beta surface used by this
  repository.
- Cross-language rename depends on the symbol graph. Missing or stale graph
  entries mean the gateway falls back to the underlying language servers.
- Binary upload/download support is raw bytes only; multipart form upload,
  WebSocket duplex transports, richer date and decimal decoding, and
  compatibility-preserving wire renames are future work.
- Generated files are hidden under `target/api-contract` by design. If your
  editor or TypeScript server cannot resolve imports, check `tsconfig` paths and
  rerun `api gen`.
