# Effect Beta Upgrade Checklist

Use this checklist when intentionally bumping the repository-pinned Effect v4 beta.

- Update `EFFECT_VERSION` in `crates/api-gen-effect-v4/src/lib.rs`.
- Update the exact `effect` dependency in `npm/effect-runtime/package.json`.
- Refresh `npm/package-lock.json` from the `npm/` workspace with `npm install --package-lock-only`.
- Review `npm/effect-runtime/src/compat.ts` before changing generated output.
- Update generator expectations and generated TypeScript fixture snapshots only after the compat surface is correct.
- Run `npm test` from `npm/`.
- Run `cargo test -p api-gen-effect-v4 generated_package_typecheck_fixture_compiles_against_pinned_effect_beta` before merging the bump.
- Run `cargo test --workspace`.
