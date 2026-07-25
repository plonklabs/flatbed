#!/usr/bin/env bash
# Prove the generated TypeScript client works end-to-end against a live flatbed
# server, with no Rust or flatc in the consumer loop:
#
#   1. boot examples/openapi
#   2. regenerate the committed client from the live spec and diff it — a
#      staleness gate, so the checked-in client can't drift from the schema
#   3. build the example and run it against the server, exercising both the
#      FlatBuffer and JSON wire formats (main.ts asserts the decoded responses)
#
# Requires cargo + flatc (to build the example server) and node/npm. The
# consumer half (generate → tsc → run) needs neither Rust nor flatc.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

if ! command -v npm >/dev/null 2>&1; then
  echo "verify-fb-client-npm: node/npm not found — skipping."
  exit 0
fi

EXAMPLE="clients/ts/examples/openapi-consumer"
GENERATED="$EXAMPLE/src/generated"
BASE="http://localhost:8080"
LOG="$(mktemp)"

echo "verify-fb-client-npm: building examples/openapi…"
cargo build --quiet --manifest-path examples/openapi/Cargo.toml
examples/openapi/target/debug/flatbed-example-openapi >"$LOG" 2>&1 &
SERVER_PID=$!
cleanup() { kill "$SERVER_PID" 2>/dev/null || true; }
trap cleanup EXIT

for _ in $(seq 1 60); do
  curl -sf "$BASE/openapi.json" >/dev/null 2>&1 && break
  sleep 0.5
done
curl -sf "$BASE/openapi.json" >/dev/null 2>&1 \
  || { echo "server did not become ready:" >&2; cat "$LOG" >&2; exit 1; }

echo "verify-fb-client-npm: installing the workspace…"
npm ci --silent --no-audit --no-fund

# The example imports @plonklabs/flatbed-client's built dist, so build it first.
npm run -w @plonklabs/flatbed-client build

# Staleness gate: regenerate from the live spec and diff the committed client.
echo "verify-fb-client-npm: checking the committed generated client is current…"
gen="$(mktemp -d)"
( cd clients/ts/flatbed-client && node --import tsx src/cli.ts generate --server "$BASE" --out "$gen" )
if ! diff -r "$gen" "$GENERATED" >/dev/null; then
  echo "error: committed client in $GENERATED is out of date for examples/openapi." >&2
  echo "Regenerate (with the server running):" >&2
  echo "  node --import tsx clients/ts/flatbed-client/src/cli.ts generate --server $BASE --out $GENERATED" >&2
  diff -r "$gen" "$GENERATED" >&2 || true
  exit 1
fi

# Build and run the example against the live server; main.ts asserts the results.
echo "verify-fb-client-npm: building + running the example…"
npm run -w @plonklabs/flatbed-openapi-example build
FLATBED_BASE_URL="$BASE" npm run -w @plonklabs/flatbed-openapi-example start

echo "verify-fb-client-npm: OK — generated client round-trips against the live server (FlatBuffer + JSON)."
