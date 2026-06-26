#!/usr/bin/env bash
# Verify the committed FlatBuffer codegen matches the `.fbs` schemas.
#
# flatc output is byte-level sensitive to the compiler version, so this
# first asserts the locally installed flatc matches `.flatc-version`; a
# version mismatch produces diffs that look like staleness but aren't.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

bash scripts/check-flatc-version.sh

SCHEMAS="crates/flatbed/schemas"
COMMITTED="crates/flatbed/src/generated"

# Regenerate into a scratch dir, then copy only the `.rs` over the
# committed tree. The CLI also writes a `.bfbs` reflection byproduct
# into its output dir, which is not committed — generating into scratch
# keeps it out of the source tree.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cargo run -q -p flatbed_build --bin flatbed -- \
  generate --schemas-dir "$SCHEMAS" --out "$tmp"
cp "$tmp"/*.rs "$COMMITTED"/

# The committed output is rustfmt-formatted; match that before diffing
# so formatting alone never reads as drift.
cargo fmt --all

if ! git diff --quiet -- "$COMMITTED"; then
  echo "error: generated code in $COMMITTED is out of date." >&2
  echo "Regenerate and commit:" >&2
  echo "  FLATBED_GENERATE=1 cargo build -p flatbed && cargo fmt --all" >&2
  git --no-pager diff --stat -- "$COMMITTED" >&2
  exit 1
fi

echo "Generated code is up to date."
