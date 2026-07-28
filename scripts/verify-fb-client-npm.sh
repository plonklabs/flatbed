#!/usr/bin/env bash
# Prove the generated TypeScript client works end-to-end against a live flatbed
# server, with no Rust or flatc in the consumer loop:
#
#   1. boot examples/openapi
#   2. regenerate the committed client from the live spec and diff it — a
#      staleness gate, so the checked-in client can't drift from the schema
#   3. build the example and run it against the server over both the FlatBuffer
#      and JSON wire formats, exiting non-zero if a decoded response is wrong
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
PORT=8080
BASE="http://127.0.0.1:$PORT"
LOG="$(mktemp)"
gen=""
SERVER_PID=""
cleanup() {
  local rc=$?
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null || true
  # Surface the server log if the run failed after the server started, otherwise
  # a late crash or a mismatched response leaves no trace.
  [ "$rc" -ne 0 ] && [ -s "$LOG" ] && { echo "--- server log ---" >&2; cat "$LOG" >&2; }
  rm -f "$LOG"
  [ -n "$gen" ] && rm -rf "$gen" || true
  return "$rc"
}
trap cleanup EXIT

# A pre-existing listener on the port would answer the readiness probe below, so
# the test would run against the wrong server while our binary fails to bind.
if (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; then
  echo "verify-fb-client-npm: something is already listening on port $PORT — free it first." >&2
  exit 1
fi

echo "verify-fb-client-npm: building examples/openapi…"
cargo build --quiet --manifest-path examples/openapi/Cargo.toml
examples/openapi/target/debug/flatbed-example-openapi >"$LOG" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 60); do
  kill -0 "$SERVER_PID" 2>/dev/null || break
  curl -sf "$BASE/openapi.json" >/dev/null 2>&1 && break
  sleep 0.5
done
if ! kill -0 "$SERVER_PID" 2>/dev/null; then
  echo "verify-fb-client-npm: server process exited during startup" >&2
  exit 1
fi
curl -sf "$BASE/openapi.json" >/dev/null 2>&1 \
  || { echo "verify-fb-client-npm: server did not become ready within 30s" >&2; exit 1; }

echo "verify-fb-client-npm: installing the workspace…"
npm ci --silent --no-audit --no-fund

npm run -w @plonklabs/flatbed-client build

echo "verify-fb-client-npm: checking the committed generated client is current…"
gen="$(mktemp -d)"
( cd clients/ts/flatbed-client && node --import tsx src/cli.ts generate --server "$BASE" --out "$gen" ) \
  || { echo "verify-fb-client-npm: generate failed (see above)" >&2; exit 1; }
if ! diff -r "$gen" "$GENERATED" >/dev/null; then
  echo "error: committed client in $GENERATED is out of date for examples/openapi." >&2
  echo "Regenerate (with the server running):" >&2
  echo "  node --import tsx clients/ts/flatbed-client/src/cli.ts generate --server $BASE --out $GENERATED" >&2
  diff -r "$gen" "$GENERATED" >&2 || true
  exit 1
fi

echo "verify-fb-client-npm: building + running the example…"
npm run -w @plonklabs/flatbed-openapi-example build
FLATBED_BASE_URL="$BASE" npm run -w @plonklabs/flatbed-openapi-example start

echo "verify-fb-client-npm: OK — generated client round-trips against the live server (FlatBuffer + JSON)."
