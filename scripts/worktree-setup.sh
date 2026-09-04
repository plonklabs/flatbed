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

if command -v fleet >/dev/null 2>&1; then
    fleet upgrade
    fleet workspace sync
    fleet ls
else
    echo "fleet not on PATH — install it, then run: fleet upgrade && fleet workspace sync" >&2
fi
