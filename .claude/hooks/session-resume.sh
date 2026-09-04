#!/usr/bin/env bash
# SessionStart (startup + compact): injects the fleet ledger's ground truth
# into a fresh or compacted session so it reads its operational position —
# seats, gate frontiers, open watches with resume commands — from the store,
# not from a summary that may have drifted. `fleet resume --hook` is silent
# when there is no open work, so one-off sessions inherit nothing.
set -euo pipefail

# Never block session start: if fleet is not built/on PATH, inject nothing.
command -v fleet >/dev/null 2>&1 || exit 0

root="${CLAUDE_PROJECT_DIR:-.}"

# A worker session runs from its worktree (…/worktrees/flatbed<N>); scope the
# render to that seat so a worker hydrates only its own frontier. The
# orchestrator runs from the main checkout, where no slot matches and the
# render spans every seat.
seat=""
case "$root" in
  */worktrees/*)
    slot="$(basename "$root")"
    digits="${slot//[!0-9]/}"
    [ -n "$digits" ] && seat="--seat $digits"
    ;;
esac

# shellcheck disable=SC2086
ground_truth="$(cd "$root" && fleet resume --hook $seat 2>/dev/null || true)"

[ -z "$ground_truth" ] && exit 0

jq -n --arg text "$ground_truth" \
  '{hookSpecificOutput: {hookEventName: "SessionStart", additionalContext: $text}}'
