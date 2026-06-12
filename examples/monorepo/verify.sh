#!/usr/bin/env bash
# Monorepo verification: cross-crate type extraction, external types, and
# nested router prefixes.
set -euo pipefail
cd "$(dirname "$0")"

TYPEWELD=${TYPEWELD:-cargo run --quiet --manifest-path ../../Cargo.toml -p typeweld-cli --}

echo "==> the server compiles"
cargo check --quiet -p monorepo-server

echo "==> bindings generate without cargo"
$TYPEWELD generate --root .

PKG=target/typeweld/packages/@workspace/monorepo-api
grep -q "export const Widget" "$PKG/schemas.ts"      # cross-crate type
grep -q "export const Part" "$PKG/schemas.ts"        # transitively reachable
grep -q '"/api/v1/widgets/{id}"' "$PKG/endpoints/widgets.ts"  # nest prefix applied
grep -q "WidgetNotFound" "$PKG/errors.ts"

echo "==> monorepo verification passed"
