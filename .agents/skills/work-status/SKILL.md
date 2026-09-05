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

1. **GitHub labels + PRs** — the source of truth, read over REST
   (`gh api repos/{owner}/{repo}/...`; the `gh issue`/`gh pr` porcelain wraps
   GraphQL and is forbidden repo-wide). Issues filtered by
   `worker:🤖flatbedN` / `state:*` / `✅ ready`; open pulls plus
   `commits/{sha}/check-runs` for check states.
2. **The `fleet` ledger** (`fleet ls`) — seat occupancy, holds, and queues.
   If it disagrees with GitHub, GitHub wins for work state; note the drift
   in the output. Live agent handles come from the harness live agent
   enumeration, not the ledger.
3. **The Project board** — not read; it's a mirror. If the render exposes a
   mismatch with it, flag that the board needs a `fleet board sync`.

## Gathering

```bash
# per seat: owned issues and their states — scoped to this orchestrator
gh api repos/plonklabs/flatbed/issues -X GET \
  -f state=open -f labels="orchestrator:$PLONK_AGENT_ID,worker:🤖flatbedN" \
  --jq '.[] | {number, title, url: .html_url, labels: [.labels[].name]}'
# dispatch-ready backlog
gh api repos/plonklabs/flatbed/issues -X GET \
  -f state=open -f labels="orchestrator:$PLONK_AGENT_ID,✅ ready" \
  --jq '.[] | {number, title, url: .html_url}'
# in-flight PRs, then their check states at head
gh api "repos/plonklabs/flatbed/pulls?state=open" \
  --jq '.[] | {number, title, url: .html_url, draft, head: .head.sha}'
gh api "repos/plonklabs/flatbed/commits/<head-sha>/check-runs" \
  --jq '.check_runs[] | "\(.name) \(.status) \(.conclusion // "")"'
# seat occupancy and queues
fleet ls
# drift findings
fleet doctor || true
```

Two properties of these endpoints bite silently. Label filters go through
`-X GET -f`, never a literal `?labels=...` query string: this repo's labels
carry emoji and spaces, and an unencoded query string comes back as an HTML
error page (`invalid character '<'`) rather than an empty list. And the issues
endpoint returns pull requests as issues too — drop any entry carrying a
`pull_request` key before counting the backlog.

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
