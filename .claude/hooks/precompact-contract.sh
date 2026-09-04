#!/usr/bin/env bash
# Injects the Session economy > compaction-priority contract from the
# orchestrator skill (the single written source) into the compaction
# summarizer's context. Re-reads the skill live on every fire, so there is
# no second copy of the contract to drift.
set -euo pipefail

root="${CLAUDE_PROJECT_DIR:-.}"
skill="$root/.agents/skills/orchestrator/SKILL.md"

contract=$(awk '/^## Session economy$/{flag=1; print; next} /^## /{if(flag) exit} flag' "$skill" 2>/dev/null || true)

if [ -z "$contract" ]; then
  contract="Session economy section not found in .agents/skills/orchestrator/SKILL.md — read that file for the compaction-priority contract before summarizing."
fi

jq -n --arg text "$contract" '{hookSpecificOutput: {hookEventName: "PreCompact", additionalContext: $text}}'
