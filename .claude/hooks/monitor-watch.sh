#!/usr/bin/env bash
# PostToolUse (Monitor): auto-registers an armed Monitor as a record-only watch
# in the fleet ledger, so a restarted session can diff the registry against its
# live monitors and re-arm any that did not survive. Translates the Claude
# Monitor payload into fleet's runtime-neutral watch descriptor; fleet never
# parses a Claude-specific shape. Never blocks the tool call.
set -euo pipefail

command -v fleet >/dev/null 2>&1 || exit 0

payload="$(cat)"

# Only Monitor arms register; the settings matcher already scopes this, but a
# stray invocation for another tool records nothing.
[ "$(jq -r '.tool_name // ""' <<<"$payload" 2>/dev/null)" = "Monitor" ] || exit 0

# The Monitor's re-arm basis is its command (bash/poll monitors) or its
# WebSocket url (ws monitors); the description is the human-readable target.
descriptor="$(jq -c '{
  kind: "monitor",
  target: (.tool_input.description // ""),
  resume_command: (.tool_input.command // .tool_input.ws.url // ""),
  status: "armed"
}' <<<"$payload" 2>/dev/null || true)"

[ -z "$descriptor" ] && exit 0

# Nothing to re-arm if neither a target nor a command survived extraction.
target="$(jq -r '.target' <<<"$descriptor")"
resume="$(jq -r '.resume_command' <<<"$descriptor")"
[ -z "$target" ] && [ -z "$resume" ] && exit 0

printf '%s' "$descriptor" | fleet watch register --from-json >/dev/null 2>&1 || true
exit 0
