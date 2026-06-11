#!/usr/bin/env bash
# End-to-end verification: generate bindings, type-check and run the Effect
# client against the live Axum server, and prove the unused-endpoint lint.
set -euo pipefail
cd "$(dirname "$0")"

TYPEWELD=${TYPEWELD:-cargo run --quiet --manifest-path ../../Cargo.toml -p typeweld-cli --}

echo "==> generate bindings (static extraction, no cargo)"
$TYPEWELD generate --root .

echo "==> build and start the server"
cargo build --quiet -p e2e-server
./target/debug/e2e-server &
SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null || true' EXIT
for _ in $(seq 1 50); do
  if curl -sf "http://127.0.0.1:39123/users?limit=1" > /dev/null 2>&1; then break; fi
  sleep 0.2
done

echo "==> install, type-check, and test the Effect client"
(cd app && npm install --no-audit --no-fund --silent && npm run typecheck && npm test)

echo "==> unused-endpoint lint stays quiet while everything is used"
if $TYPEWELD check --root . --unused 2>&1 | grep -q "unused-endpoint"; then
  echo "unexpected unused-endpoint lint" >&2
  exit 1
fi

echo "==> unused-endpoint lint fires for an unreferenced endpoint"
cat > server/src/unused_probe.rs <<'RS'
use typeweld::{api, Json, Path};

/// Probe endpoint that no TypeScript code references.
#[api(get, "/probe/{id}")]
pub async fn unused_probe(id: Path<i32>) -> Json<crate::User> {
    Json(crate::User { id: *id, display_name: String::new(), bio: None })
}
RS
echo "mod unused_probe;" >> server/src/main.rs
cleanup_probe() {
  git checkout -- server/src/main.rs 2>/dev/null || sed -i '$ d' server/src/main.rs
  rm -f server/src/unused_probe.rs
}
trap 'cleanup_probe; kill $SERVER_PID 2>/dev/null || true' EXIT
if ! $TYPEWELD check --root . --unused 2>&1 | grep -q "unused-endpoint"; then
  echo "expected unused-endpoint lint did not fire" >&2
  exit 1
fi
cleanup_probe
trap 'kill $SERVER_PID 2>/dev/null || true' EXIT

echo "==> e2e verification passed"
