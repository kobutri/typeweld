#!/usr/bin/env sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
APP_DIR="$SCRIPT_DIR/app"
WITHOUT_AUDIT_DIR="$REPO_ROOT/target/examples/e2e-axum-effect/without-audit"
WITHOUT_AUDIT_LOG="$REPO_ROOT/target/examples/e2e-axum-effect/without-audit-check.log"

cd "$REPO_ROOT"

echo "1. Checking the Axum server crate"
cargo check -p e2e-axum-effect-server

echo "2. Running cargo typeweld check with generated TS typecheck and usage lints"
cargo run -q -p typeweld-cli --bin typeweld -- check \
  --package e2e-axum-effect-server \
  --target-dir target \
  --ts-dir examples/e2e-axum-effect/app/src \
  --deny-unused-endpoints

echo "3. Typechecking the TypeScript Effect app"
npm --prefix "$APP_DIR" run typecheck

echo "4. Running the generated Effect client against the Axum server"
npm --prefix "$APP_DIR" run test:runtime

echo "5. Proving removed TS usage causes an unused-endpoint diagnostic"
rm -rf "$WITHOUT_AUDIT_DIR"
mkdir -p "$WITHOUT_AUDIT_DIR/src"
cp "$APP_DIR"/src/*.ts "$WITHOUT_AUDIT_DIR/src/"
rm "$WITHOUT_AUDIT_DIR/src/audit-usage.ts"

if cargo run -q -p typeweld-cli --bin typeweld -- check \
  --package e2e-axum-effect-server \
  --target-dir target \
  --ts-dir "$WITHOUT_AUDIT_DIR/src" \
  --deny-unused-endpoints >"$WITHOUT_AUDIT_LOG" 2>&1; then
  echo "expected typeweld check to fail after removing audit endpoint usage" >&2
  exit 1
fi

if ! grep -q "unused endpoint diagnostic" "$WITHOUT_AUDIT_LOG"; then
  cat "$WITHOUT_AUDIT_LOG" >&2
  echo "expected unused endpoint diagnostic in typeweld check output" >&2
  exit 1
fi

echo "e2e Axum + Effect example works"
