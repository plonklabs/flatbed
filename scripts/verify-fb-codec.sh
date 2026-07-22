#!/usr/bin/env bash
# Prove the generated TypeScript FlatBuffer codecs are byte-compatible with the
# Rust `flatbuffers` implementation, in both directions — for both generators:
#
#   Rust codegen (`flatbed gen-fb-plugin`, flatc reflection in Rust)
#   npm codegen  (`@plonklabs/flatbed-client`, reflection read from the `.bfbs`)
#
# For each representative type (strings, 64-bit ints, bools, nested tables,
# vectors of tables/strings, enums and vectors of enums):
#   Rust encodes  → each TS codec decodes and asserts   (Rust → TS)
#   each TS codec encodes → Rust decodes and asserts     (TS → Rust)
#   the two TS codecs produce byte-identical output      (npm ≡ Rust codegen)
#
# A value only survives if every independent implementation agrees on the exact
# bytes. Requires `node`/`npx` and the pinned `flatc`; skips when node is absent.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

if ! command -v npx >/dev/null 2>&1; then
  echo "verify-fb-codec: node/npx not found — skipping the codec round-trip."
  exit 0
fi

TYPES="TestResponse UserRequest AddressBook LogEvent Defaulted"
SCHEMAS="crates/flatbed/schemas"
BFBS="crates/flatbed/src/generated/test.bfbs"
PKG="clients/ts/flatbed-client"
WORK="$(mktemp -d)"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

printf '{"paths":{}}' > "$WORK/empty.json"

driver() {
  cat >"$1/roundtrip.ts" <<'TS'
import * as codec from "./codec.js";
import { Severity } from "./types.js";

const address = () => ({ street: "1 Analytical Way", city: "London", zip_code: 12345 });

function sample(ty: string): any {
  switch (ty) {
    case "TestResponse": return { message: "pong", value: 9000000000000000000n, success: true };
    case "UserRequest": return { name: "Ada", age: 36, address: address() };
    case "AddressBook": return { owner: "Ada", addresses: [address(), address()], contact_names: ["Bob", "Carol"] };
    case "LogEvent": return { message: "disk full", severity: Severity.Error, history: [Severity.Info, Severity.Warning, Severity.Error] };
    case "Defaulted": return { count: 25, flag: true, ratio: 1.5, level: Severity.Warning };
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
}

cat >"$WORK/tsconfig-base.json" <<'JSON'
{ "compilerOptions": { "target": "es2020", "module": "es2020",
  "moduleResolution": "node", "outDir": "dist", "strict": true, "skipLibCheck": true },
  "include": ["src/**/*.ts"] }
JSON

echo "verify-fb-codec: generating the Rust codec (gen-fb-plugin)…"
cargo build --quiet -p flatbed_build --bin flatbed
mkdir -p "$WORK/rust/src"
./target/debug/flatbed gen-fb-plugin --openapi "$WORK/empty.json" \
  --schemas-dir "$SCHEMAS" --out "$WORK/rust/src" >/dev/null
driver "$WORK/rust/src"

echo "verify-fb-codec: generating the npm codec (@plonklabs/flatbed-client)…"
( cd "$PKG" && npm ci --silent --no-audit --no-fund >"$WORK/npm-ci.log" 2>&1 ) \
  || { echo "npm ci failed ($PKG):" >&2; cat "$WORK/npm-ci.log" >&2; exit 1; }
mkdir -p "$WORK/npm/src"
ABS_BFBS="$(pwd)/$BFBS"
# The CLI runs from the package dir so its `tsx` loader resolves; inputs and
# output are absolute since the cwd changes.
( cd "$PKG" && node --import tsx src/cli.ts generate \
    --openapi "$WORK/empty.json" --schema "$ABS_BFBS" --out "$WORK/npm/gen" >"$WORK/npm-gen.log" 2>&1 ) \
  || { echo "npm codec generation failed:" >&2; cat "$WORK/npm-gen.log" >&2; exit 1; }
# The round-trip needs the FlatBuffer codec and the type module (enums are
# runtime value imports, not type-erased); the JSON codec is copied only for the
# tsc type-check.
cp "$WORK/npm/gen/codec.ts" "$WORK/npm/gen/json-codec.ts" "$WORK/npm/gen/types.ts" "$WORK/npm/src/"
driver "$WORK/npm/src"

echo "verify-fb-codec: installing flatbuffers + typescript…"
# Match the range the published client depends on, so a major bump can't leave
# this harness silently proving compatibility against the old wire runtime.
FB_VER="$(node -p "require('./$PKG/package.json').dependencies.flatbuffers")"
for dir in rust npm; do
  cat >"$WORK/$dir/package.json" <<'JSON'
{ "name": "fb-codec-verify", "private": true, "type": "module" }
JSON
  cp "$WORK/tsconfig-base.json" "$WORK/$dir/tsconfig.json"
  ( cd "$WORK/$dir" && npm install --silent --no-audit --no-fund "flatbuffers@${FB_VER}" typescript@5 @types/node@20 >npm.log 2>&1 ) \
    || { echo "npm install failed ($dir):" >&2; cat "$WORK/$dir/npm.log" >&2; exit 1; }
  ( cd "$WORK/$dir" && npx --yes tsc )
done

rust() { cargo run --quiet -p flatbed --example fb_roundtrip --features openapi -- "$@"; }
ts_rust() { node "$WORK/rust/dist/roundtrip.js" "$@"; }
ts_npm() { node "$WORK/npm/dist/roundtrip.js" "$@"; }

for ty in $TYPES; do
  rust_hex="$(rust encode "$ty")"
  [ "$(ts_rust decode "$ty" "$rust_hex")" = "ok" ] || { echo "FAIL: Rust codec TS could not decode Rust $ty" >&2; exit 1; }
  [ "$(ts_npm decode "$ty" "$rust_hex")" = "ok" ]  || { echo "FAIL: npm codec TS could not decode Rust $ty" >&2; exit 1; }
  rust_ts_hex="$(ts_rust encode "$ty")"
  npm_ts_hex="$(ts_npm encode "$ty")"
  [ "$npm_ts_hex" = "$rust_ts_hex" ] || { echo "FAIL: npm codec bytes differ from the Rust codec for $ty" >&2; exit 1; }
  [ "$(rust decode "$ty" "$npm_ts_hex")" = "ok" ] || { echo "FAIL: Rust could not decode npm TS $ty" >&2; exit 1; }
  echo "  ✓ $ty — Rust ↔ Rust-codec ↔ npm-codec all byte-compatible"
done

echo "verify-fb-codec: OK — both generated codecs round-trip against Rust byte-for-byte."
