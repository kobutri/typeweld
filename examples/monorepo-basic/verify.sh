#!/usr/bin/env sh
set -eu

# This script is the "does the example actually work?" check.
# It intentionally follows the same steps described in README.md instead of
# calling hidden test helpers.

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)

cd "$REPO_ROOT"

echo "1. Checking the Rust example compiles"
cargo check -p monorepo-basic

echo "2. Emitting the Rust API contract JSON"
mkdir -p target/api-contract
cargo run -q -p monorepo-basic > target/api-contract/server-api.json

echo "3. Running the full API check workflow"
cargo run -q -p typeweld-cli --bin typeweld -- check \
  --contract target/api-contract/server-api.json \
  --target-dir target \
  --ts-dir examples/monorepo-basic/app/src

echo "4. Typechecking the TypeScript app against the generated package"
npm --prefix examples/monorepo-basic/app run typecheck

echo "monorepo-basic works"
