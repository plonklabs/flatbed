#!/usr/bin/env bash
# Verify the committed flatc `--ts` reflection bindings match the pinned flatc.
#
# The TS client reads a served `.bfbs` through FlatBuffer reflection bindings
# vendored under clients/ts/flatbed-client/src/generate/fbs-reflection/. They're
# `flatc --ts` output over the flatbuffers `reflection.fbs`, so they must be
# regenerable byte-for-byte from the pinned compiler; a drifted checkout would
# silently parse `.bfbs` buffers against stale bindings.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

bash scripts/check-flatc-version.sh

VER="$(cat .flatc-version | tr -d '[:space:]')"
COMMITTED="clients/ts/flatbed-client/src/generate/fbs-reflection"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL \
  "https://raw.githubusercontent.com/google/flatbuffers/v${VER}/reflection/reflection.fbs" \
  -o "$tmp/reflection.fbs"
flatc --ts --gen-all -o "$tmp/gen" "$tmp/reflection.fbs"

if ! diff -r "$tmp/gen" "$COMMITTED" >/dev/null; then
  echo "error: committed reflection bindings in $COMMITTED are out of date for flatc v$VER." >&2
  echo "Regenerate and commit:" >&2
  echo "  flatc --ts --gen-all -o $COMMITTED <reflection.fbs for v$VER>" >&2
  diff -r "$tmp/gen" "$COMMITTED" >&2 || true
  exit 1
fi

echo "Reflection bindings are up to date."
