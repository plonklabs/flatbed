#!/usr/bin/env bash
# Assert the locally-installed `flatc` matches the version pinned in
# `.flatc-version`. Sourced by both `scripts/check-generated.sh` (CI
# gate) and the Makefile's `generate` target so developer machines and
# CI runners can't drift independently.
#
# Different flatc versions produce different byte-level output for the
# same `.fbs` schema (field ordering, comment phrasing, helper
# signatures). A runner with a drifted flatc reports "stale generated
# code" on every PR even when the source tree is correct.
#
# Bumping the version is a coordinated change: edit `.flatc-version`
# AND `images/arc-base/Dockerfile` together so the runner image and
# the script agree.
#
# Usage:
#   bash scripts/check-flatc-version.sh   # exits 0 if version matches, 1 otherwise

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

EXPECTED="$(cat "$REPO_ROOT/.flatc-version" | tr -d '[:space:]')"

# Prepend the version-pinned flatc cache to PATH when present, so it takes
# precedence over whatever flatc the host has installed. The cache lives
# under the main checkout, keyed off the common .git dir so every worker
# worktree resolves the same shared location.
COMMON_GIT_DIR="$(git -C "$REPO_ROOT" rev-parse --git-common-dir 2>/dev/null || true)"
case "$COMMON_GIT_DIR" in
  /*) ;;
  ?*) COMMON_GIT_DIR="$REPO_ROOT/$COMMON_GIT_DIR" ;;
esac
if [ -n "$COMMON_GIT_DIR" ]; then
  CACHE_DIR="$(cd "$COMMON_GIT_DIR/.." && pwd)/.cache/flatc/$EXPECTED"
  if [ -x "$CACHE_DIR/flatc" ]; then
    PATH="$CACHE_DIR:$PATH"
  fi
fi

ACTUAL="$(flatc --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' || true)"

if [ -z "$ACTUAL" ]; then
  echo "ERROR: flatc not found on PATH; install flatc v$EXPECTED and retry." >&2
  exit 1
fi

if [ "$EXPECTED" != "$ACTUAL" ]; then
  echo "ERROR: flatc version mismatch." >&2
  echo "  Expected (per .flatc-version): $EXPECTED" >&2
  echo "  Got (per \`flatc --version\`):  $ACTUAL" >&2
  echo "" >&2
  echo "Generated FlatBuffer output is version-sensitive; bumping one without the" >&2
  echo "other produces phantom \"generated code is stale\" failures on PRs that" >&2
  echo "didn't touch schemas. Either install flatc v$EXPECTED locally or bump" >&2
  echo ".flatc-version and images/arc-base/Dockerfile together." >&2
  exit 1
fi
