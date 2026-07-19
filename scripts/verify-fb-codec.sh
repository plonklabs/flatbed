#!/usr/bin/env bash
# Prove the generated TypeScript FlatBuffer codec is byte-compatible with the
# Rust `flatbuffers` implementation, in both directions.
#
# For each representative type (covering strings, 64-bit ints, bools, nested
# tables, vectors of tables/strings, enums and vectors of enums):
#   Rust encodes  → TS decodes and asserts   (Rust → TS wire compatibility)
#   TS encodes    → Rust decodes and asserts  (TS → Rust wire compatibility)
#
# A value only survives if the two independent implementations agree on the
# exact bytes. Requires `node`/`npx` and the pinned `flatc`; skips cleanly when
# node tooling is absent.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

if ! command -v npx >/dev/null 2>&1; then
  echo "verify-fb-codec: node/npx not found — skipping the codec round-trip."
  exit 0
fi

TYPES="TestResponse UserRequest AddressBook LogEvent"
WORK="$(mktemp -d)"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

echo "verify-fb-codec: generating the TS codec from test.fbs…"
cargo build --quiet -p flatbed_build --bin flatbed
./target/debug/flatbed gen-fb-plugin \
  --openapi <(printf '{"paths":{}}') \
  --schemas-dir crates/flatbed/schemas \
  --out "$WORK/src" >/dev/null

cat >"$WORK/package.json" <<'JSON'
{ "name": "fb-codec-verify", "private": true, "type": "module" }
JSON
cat >"$WORK/tsconfig.json" <<'JSON'
{ "compilerOptions": { "target": "es2020", "module": "es2020",
  "moduleResolution": "node", "outDir": "dist", "strict": true, "skipLibCheck": true },
  "include": ["src/**/*.ts"] }
JSON

# The Node mirror of the Rust samples in examples/fb_roundtrip.rs.
cat >"$WORK/src/roundtrip.ts" <<'TS'
import * as codec from "./codec.js";
import { Severity } from "./types.js";

const address = () => ({ street: "1 Analytical Way", city: "London", zip_code: 12345 });

function sample(ty: string): any {
  switch (ty) {
    case "TestResponse": return { message: "pong", value: 9000000000000000000n, success: true };
    case "UserRequest": return { name: "Ada", age: 36, address: address() };
    case "AddressBook": return { owner: "Ada", addresses: [address(), address()], contact_names: ["Bob", "Carol"] };
    case "LogEvent": return { message: "disk full", severity: Severity.Error, history: [Severity.Info, Severity.Warning, Severity.Error] };
    default: throw new Error("unknown type " + ty);
  }
}

const canon = (v: any): string =>
  JSON.stringify(v, (_k, x) =>
    typeof x === "bigint" ? `${x}n`
    : x && typeof x === "object" && !Array.isArray(x)
      ? Object.fromEntries(Object.entries(x).filter(([, y]) => y !== undefined).sort())
      : x);

const toHex = (b: Uint8Array) => [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
const fromHex = (s: string) => Uint8Array.from(s.match(/../g)!.map((h) => parseInt(h, 16)));

const [, , mode, ty, hex] = process.argv;
const enc = (codec as any)[`encode${ty}Root`];
const dec = (codec as any)[`decode${ty}Root`];

if (mode === "encode") {
  process.stdout.write(toHex(enc(sample(ty))));
} else if (mode === "decode") {
  const got = dec(fromHex(hex));
  if (canon(got) !== canon(sample(ty))) {
    console.error(`mismatch for ${ty}:\n  got: ${canon(got)}\n  exp: ${canon(sample(ty))}`);
    process.exit(1);
  }
  process.stdout.write("ok");
} else {
  throw new Error("usage: roundtrip encode|decode <Type> [hex]");
}
TS

echo "verify-fb-codec: installing flatbuffers + typescript…"
( cd "$WORK" && npm install --silent --no-audit --no-fund flatbuffers@25 typescript@5 @types/node@20 >npm.log 2>&1 ) \
  || { echo "npm install failed:" >&2; cat "$WORK/npm.log" >&2; exit 1; }

echo "verify-fb-codec: type-checking + compiling the codec…"
( cd "$WORK" && npx --yes tsc )

rust() { cargo run --quiet -p flatbed --example fb_roundtrip --features openapi -- "$@"; }
node_rt() { node "$WORK/dist/roundtrip.js" "$@"; }

for ty in $TYPES; do
  rust_hex="$(rust encode "$ty")"
  [ "$(node_rt decode "$ty" "$rust_hex")" = "ok" ] || { echo "FAIL: TS could not decode Rust $ty" >&2; exit 1; }
  ts_hex="$(node_rt encode "$ty")"
  [ "$(rust decode "$ty" "$ts_hex")" = "ok" ] || { echo "FAIL: Rust could not decode TS $ty" >&2; exit 1; }
  echo "  ✓ $ty — Rust↔TS byte-compatible"
done

echo "verify-fb-codec: OK — the generated codec round-trips against Rust for every type."
