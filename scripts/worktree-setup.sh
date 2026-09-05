#!/usr/bin/env bash
# Create the fixed worker seats: worktrees/flatbed1..4, each parked on its
# dev/slot-N branch, then register them in the fleet ledger. Idempotent —
# existing worktrees are left untouched. flatbed4 is reserved for the user
# (`fleet hold 4 --user` after the sync); the orchestrator dispatches only
# to flatbed1..3.
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"

for n in 1 2 3 4; do
    dir="worktrees/flatbed$n"
    branch="dev/slot-$n"
    if [ -d "$dir" ]; then
        echo "$dir exists — skipped"
        continue
    fi
    git branch --force "$branch" origin/main 2>/dev/null \
        || git branch "$branch" origin/main
    git worktree add "$dir" "$branch"
    echo "$dir created on $branch"
done

# Fetch the `.flatc-version`-pinned flatc binary into a cache under the main
# checkout: seats otherwise skip scripts/check-generated.sh whenever the
# host's own flatc has drifted from the pin. check-flatc-version.sh resolves
# this same cache from any worktree (via the shared common .git dir) and
# prefers it on PATH.
flatc_version="$(tr -d '[:space:]' <.flatc-version)"
cache_dir=".cache/flatc/$flatc_version"
if [ -x "$cache_dir/flatc" ]; then
    echo "flatc $flatc_version already cached at $cache_dir"
elif [ -z "$flatc_version" ]; then
    echo "warning: .flatc-version is empty — skipping flatc cache fetch" >&2
else
    case "$(uname -s)-$(uname -m)" in
        Darwin-arm64)  asset=Mac.flatc.binary.zip ;;
        Darwin-x86_64) asset=MacIntel.flatc.binary.zip ;;
        Linux-*)       asset=Linux.flatc.binary.g++-13.zip ;;
        *)             asset="" ;;
    esac
    if [ -z "$asset" ]; then
        echo "warning: no known flatc release asset for $(uname -s)-$(uname -m) — skipping cache fetch" >&2
    else
        tmp="$(mktemp -d)"
        url="https://github.com/google/flatbuffers/releases/download/v${flatc_version}/${asset}"
        if curl -fsSL "$url" -o "$tmp/flatc.zip" \
            && unzip -q "$tmp/flatc.zip" -d "$tmp" \
            && mkdir -p "$cache_dir" \
            && install -m 0755 "$tmp/flatc" "$cache_dir/flatc"; then
            echo "cached flatc $flatc_version at $cache_dir"
        else
            echo "warning: failed to fetch flatc $flatc_version from $url — skipping cache fetch" >&2
        fi
        rm -rf "$tmp"
    fi
fi

if command -v fleet >/dev/null 2>&1; then
    fleet upgrade
    fleet workspace sync
    fleet ls
else
    echo "fleet not on PATH — install it, then run: fleet upgrade && fleet workspace sync" >&2
fi
