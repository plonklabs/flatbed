# Development-agent architecture

## Orchestrator

The orchestrator aligns feature intent, creates one issue per PR-sized step
with `/spec`, and dispatches ready work to worker seats. It owns product-design
decisions, GitHub issue/label/Project state, Fleet coordination, and CI
monitoring. It never edits implementation files or owns a pull request.

## Worker

A worker receives exactly one issue and owns its branch, implementation,
tests, PR lifecycle, review remediation, and close-out. It loads the canonical
[worker role](roles/worker.md) and procedures in [skills](skills/).

## Worktree seats

`worktrees/flatbed1..4` are fixed isolated execution seats, created by
`scripts/worktree-setup.sh`. flatbed4 is reserved for the user; the
orchestrator dispatches only to flatbed1–3. Each seat has its own Cargo
target dir and its own NATS broker (`scripts/nats-broker.sh` derives the
container name and port from the worktree basename), so seats build and test
fully in parallel — there is no serialized test bench.

## Fleet ledger

`fleet` is the local SQLite-backed coordinator — one ledger shared by the
orchestrator and all seats, at `.plonk/local/fleet.db` under the main
checkout. It owns seat occupancy (`fleet push/pop --stack work`), user holds
(`fleet hold`), executor records (`fleet assign`, `fleet heartbeat`), drift
detection (`fleet doctor`), watch re-arm records (`fleet watch`), session
hydration (`fleet resume`), board reconcile (`fleet board sync`), and the
precondition-checked merge path (`fleet merge <n> --no-merge`, with the gates
declared in `.fleet/merge.toml`; its exit 0 is what authorizes the
`gh pr merge --admin --match-head-commit` that lands the commit). It also
renders every worker dispatch (`fleet brief`, spawn and re-wake). Fleet does
not spawn agents, edit GitHub work state, or decide product design.

## Durable state

GitHub issues and labels are durable work truth. The Project board is a
reconciled visual mirror; the fleet ledger is capacity/executor truth, not a
replacement for GitHub state.

## End-to-end flow

`/spec` → dispatch (orchestrator, via the fleet work stack and `fleet brief`)
→ one worker per seat → `/implement` → `/review --auto` → `fleet merge
--no-merge` → the authorized admin merge. Shared policy is in
[`AGENTS.md`](../AGENTS.md); harness adapters (`.claude/`) only map native
discovery and lifecycle capabilities.
