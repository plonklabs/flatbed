#!/usr/bin/env bash
# Prove that flatbed's generated OpenAPI spec is type-complete and consumable
# by a standard OpenAPI code generator.
#
# The Rust `openapi_schema` integration test pins the spec's *shape*; this
# closes the loop against the real ecosystem: it boots the `openapi` example,
# pulls its `/openapi.json`, generates TypeScript with `openapi-typescript`,
# and type-checks the result with `tsc`. A green run means any JSON client
# generated from the spec compiles — nested tables, enums, arrays and integer
# formats included.
#
# Requires `node`/`npx` and `flatc` (for the example's build-time codegen).
# Skips cleanly (exit 0) when node tooling is absent so Rust-only environments
# aren't blocked.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

if ! command -v npx >/dev/null 2>&1; then
  echo "verify-openapi-ts: node/npx not found — skipping the TS consumer check."
  exit 0
fi

PORT=8080
SPEC="$(mktemp)"
WORK="$(mktemp -d)"
SERVER_PID=""
cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null || true
  rm -rf "$WORK" "$SPEC"
}
trap cleanup EXIT

echo "verify-openapi-ts: building the openapi example…"
( cd examples/openapi && cargo build --quiet )

echo "verify-openapi-ts: starting the server…"
./examples/openapi/target/debug/flatbed-example-openapi >"$WORK/server.log" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:$PORT/openapi.json" -o "$SPEC" 2>/dev/null; then
    break
  fi
  sleep 0.5
done
if ! [ -s "$SPEC" ]; then
  echo "verify-openapi-ts: server never served /openapi.json" >&2
  cat "$WORK/server.log" >&2
  exit 1
fi

echo "verify-openapi-ts: generating TypeScript with openapi-typescript…"
cd "$WORK"
echo '{"name":"flatbed-openapi-verify","private":true,"type":"module"}' >package.json
# Pin the generator's major version so the grep assertions below stay stable
# across generator releases. Surface install failures before the EXIT trap
# deletes the log.
if ! npm install --silent --no-audit --no-fund \
    "openapi-typescript@7" "typescript@5" >npm.log 2>&1; then
  echo "verify-openapi-ts: npm install failed:" >&2
  cat npm.log >&2
  exit 1
fi
npx --yes openapi-typescript "$SPEC" -o types.ts

echo "verify-openapi-ts: type-checking generated output with tsc…"
npx --yes tsc --noEmit --strict types.ts

# Assert the rich features actually round-tripped into the generated types,
# not just that *something* compiled.
grep -q 'Priority: "Low" | "Medium" | "High"' types.ts \
  || { echo "expected Priority to be a string-literal union" >&2; exit 1; }
grep -q 'Tag: {' types.ts \
  || { echo "expected the nested-only Tag table to be its own type" >&2; exit 1; }
grep -q 'tags?: components\["schemas"\]\["Tag"\]\[\]' types.ts \
  || { echo "expected tags to be an array of Tag" >&2; exit 1; }

echo "verify-openapi-ts: OK — the spec generates type-checked TypeScript, rich features included."
