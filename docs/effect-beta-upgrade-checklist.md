# Effect Beta Upgrade Checklist

Use this checklist when intentionally bumping the repository-pinned Effect v4 beta.

- Update `EFFECT_VERSION` in `crates/api-gen-effect-v4/src/lib.rs`.
- Update the exact `effect` dependency in `npm/effect-runtime/package.json`.
- Refresh `npm/package-lock.json` from the `npm/` workspace with `npm install --package-lock-only`.
- Review `npm/effect-runtime/src/compat.ts` before changing generated output.
- Update generator expectations and generated TypeScript fixture snapshots only after the compat surface is correct.
- Run `npm test` from `npm/`.
- Run generated-package typecheck fixtures before merging the bump. Until the D4 fixture suite lands, run `cargo test -p api-gen-effect-v4` as the repository guard for generated output shape.
- Run `cargo test --workspace`.
