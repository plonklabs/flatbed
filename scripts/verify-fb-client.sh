#!/usr/bin/env bash
# End-to-end proof of the generated FlatBuffer client: boot a real flatbed
# service, generate the TypeScript client from its spec + schemas, and have that
# client call the service over `application/x-flatbuffers`.
#
# A green run means the whole chain works against a live server: the TS client
# encodes a request to FlatBuffer bytes, the Rust service decodes and handles
# it, encodes the response, and the client decodes it back to a typed object.
#
# Requires `node`/`npx` and the pinned `flatc`; skips cleanly when node tooling
# is absent.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

if ! command -v npx >/dev/null 2>&1; then
  echo "verify-fb-client: node/npx not found — skipping the end-to-end client check."
  exit 0
fi

PORT=8080
WORK="$(mktemp -d)"
SERVER_PID=""
cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

# A pre-existing listener on the port would answer the readiness probe below,
# so the test would run against the wrong server while our binary fails to bind.
if curl -fsS "http://127.0.0.1:$PORT/openapi.json" -o /dev/null 2>/dev/null; then
  echo "verify-fb-client: something is already listening on port $PORT — free it first." >&2
  exit 1
fi

echo "verify-fb-client: starting the openapi example service…"
( cd examples/openapi && cargo build --quiet )
./examples/openapi/target/debug/flatbed-example-openapi >"$WORK/server.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 60); do
  # Bail as soon as our process dies (e.g. failed to bind) rather than waiting
  # out the timeout against a port nothing is serving.
  kill -0 "$SERVER_PID" 2>/dev/null || break
  curl -fsS "http://127.0.0.1:$PORT/openapi.json" -o /dev/null 2>/dev/null && break
  sleep 0.5
done
if ! kill -0 "$SERVER_PID" 2>/dev/null; then
  echo "verify-fb-client: server process exited during startup" >&2
  cat "$WORK/server.log" >&2
  exit 1
fi
if ! curl -fsS "http://127.0.0.1:$PORT/openapi.json" -o /dev/null 2>/dev/null; then
  echo "verify-fb-client: server failed to start within 30s" >&2
  cat "$WORK/server.log" >&2
  exit 1
fi

echo "verify-fb-client: generating the FlatBuffer client…"
cargo build --quiet -p flatbed_build --bin flatbed
./target/debug/flatbed gen-fb-plugin \
  --server "http://127.0.0.1:$PORT" \
  --schemas-dir examples/openapi/schemas \
  --out "$WORK/src" >/dev/null

cat >"$WORK/package.json" <<'JSON'
{ "name": "fb-client-verify", "private": true, "type": "module" }
JSON
cat >"$WORK/tsconfig.json" <<'JSON'
{ "compilerOptions": { "target": "es2022", "module": "es2022",
  "moduleResolution": "node", "lib": ["es2022", "dom"], "outDir": "dist",
  "strict": true, "skipLibCheck": true }, "include": ["src/**/*.ts"] }
JSON
cat >"$WORK/src/main.ts" <<TS
import { FlatbedClient } from "./client.js";
import { Priority } from "./types.js";

const client = new FlatbedClient({ baseUrl: "http://127.0.0.1:$PORT" });

const echo = await client.postEcho({ message: "hi", times: 3, priority: Priority.Low });
if (echo.message !== "hi hi hi") {
  console.error("echo mismatch:", echo);
  process.exit(1);
}

const greet = await client.postGreet({ name: "Ada" });
if (greet.greeting !== "Hello, Ada!") {
  console.error("greet mismatch:", greet);
  process.exit(1);
}

console.log("ok");
TS

echo "verify-fb-client: installing flatbuffers + typescript…"
( cd "$WORK" && npm install --silent --no-audit --no-fund \
    flatbuffers@25 typescript@5 @types/node@20 >npm.log 2>&1 ) \
  || { echo "npm install failed:" >&2; cat "$WORK/npm.log" >&2; exit 1; }

echo "verify-fb-client: type-checking + compiling…"
( cd "$WORK" && npx --yes tsc )

echo "verify-fb-client: calling the live service over application/x-flatbuffers…"
result="$(node "$WORK/dist/main.js")"
[ "$result" = "ok" ] || { echo "verify-fb-client: client call failed" >&2; exit 1; }

echo "verify-fb-client: OK — the generated client round-trips against the live service."
