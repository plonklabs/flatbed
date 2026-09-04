---
name: work-status
description: Render a read-only snapshot of the orchestration board, pull requests, and pending decisions.
---

# /work-status — Render the Orchestration Board

## Description

Read-only snapshot of the orchestration state: what every seat is doing,
where every in-flight PR stands, and which decisions are waiting on the
user. Safe to run at any time, including mid-dispatch — it mutates nothing.

## Data sources (in precedence order)

1. **GitHub labels + PRs** — the source of truth. `gh issue list` filtered
   by `worker:🤖flatbedN` / `state:*` / `✅ ready`; `gh pr list` +
   `statusCheckRollup` for check states.
2. **The `fleet` ledger** (`fleet ls`) — seat occupancy, holds, and queues.
   If it disagrees with GitHub, GitHub wins for work state; note the drift
   in the output. Live agent handles come from the harness live agent
   enumeration, not the ledger.
3. **The Project board** — not read; it's a mirror. If the render exposes a
   mismatch with it, flag that the board needs a `fleet board sync`.

## Gathering

```bash
# per seat: owned issues and their states — scoped to this orchestrator
gh issue list --state open --label "orchestrator:$PLONK_AGENT_ID" \
  --label "worker:🤖flatbedN" --json number,title,labels,url
# in-flight PRs with check status
gh pr list --state open --search "label:orchestrator:$PLONK_AGENT_ID" \
  --json number,title,url,isDraft,statusCheckRollup
# dispatch-ready backlog
gh issue list --state open --label "orchestrator:$PLONK_AGENT_ID" \
  --label "✅ ready" --json number,title,url
# seat occupancy and queues
fleet ls
# drift findings
fleet doctor || true
```

## Rendering

1. A compact table: seat / issue / PR / state glyph / one-word phase.
2. In-flight PRs with full URLs and their check rollup in one word each.
3. The ready queue in board order.
4. Pending user decisions as a numbered list, one line each.
5. Any ledger↔GitHub drift, flagged explicitly.

## Rules

- Read-only: no label writes, no ledger writes, no board mutations.
- Full URLs on anything the user might act on.
- Drift is reported, never silently repaired here — repairs go through the
  orchestrator's Re-sync.
