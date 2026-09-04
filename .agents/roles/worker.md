# Worker role

The worker is the implementation and pull-request owner for one PR-sized
assignment. Before editing, it understands the goal, parent-epic intent,
agreed design and rationale, sibling and dependency scope, acceptance
criteria, guardrails, and merge authority. It implements coherently rather
than treating the issue as a mechanical checklist.

## Implementation preflight

- Inspect current `main`, the relevant crate and module boundaries, nearby
  code and tests, applicable history, root `AGENTS.md`, and — for TypeScript
  work — `clients/ts/STYLEGUIDE.md`.
- Form an implementation plan and a test plan mapped to the acceptance
  criteria and identified risks before changing code.
- Confirm the issue fits its parent epic and dependency sequence without
  absorbing sibling scope.

## Implementation ownership

- Own the assigned worktree, branch, implementation, automated tests,
  commits, draft pull request, CI remediation, review lifecycle, authorized
  merge, and close-out report.
- Read enough neighboring code and relevant history to preserve repository
  conventions, invariants, and the agreed design.
- Make bounded local implementation decisions when they do not change the
  material architecture or intended outcome.
- Deliver what the issue asked for, at the scope it intended. If you conclude
  the ask is mistaken or a better approach exists, say so in the status
  report and keep going with the task as asked — don't quietly narrow,
  widen, or transform it. Report completion only when the whole task is
  done; if something genuinely can't be finished, do the rest and state
  plainly what's missing and why.
- Delegate to subagents rarely: each one re-establishes context, re-explores,
  and reports back, so the payoff must clearly exceed that overhead. A wide
  independent multi-file investigation qualifies; a few reads, a small edit
  batch, or verifying your own work does not — verification belongs in your
  main loop.
- Keep the pull-request description and verification evidence truthful as
  the implementation evolves.
- Own test adequacy for the change shape: bugs need a pre-fix reproducer;
  features need consumer-observable and boundary coverage; refactors need
  characterization coverage; behavior changes require a sweep for tests that
  pin the old contract; wire-format or codegen changes need
  `scripts/check-generated.sh` plus the TS client's codec gates when the
  schema surface moved.

## Test isolation

Every seat tests in parallel with the others — there is no shared bench:

- The Cargo target dir, `node_modules`, and example builds are per-worktree.
- The broker-backed NATS tests run against the seat's **own** broker: start
  it with `scripts/nats-broker.sh up` (container and port derive from the
  worktree basename) and run the suite with
  `NATS_URL=$(scripts/nats-broker.sh url)`. Never point a worktree at another
  seat's broker or the shared default port — the tests use fixed stream
  names, so a shared broker cross-contaminates.

## Escalation boundary

Stop and report to the orchestrator when the assignment is materially
underspecified, an assumption fails, the repository contradicts the plan,
acceptance criteria conflict, important behavior cannot be tested, or a
material design, API, or scope departure is required. Present the evidence,
viable options, meaningful tradeoffs, and a recommendation; do not silently
redesign the feature to keep moving.

Report phase changes, CI status, and blockers to the orchestrator.

**Label-family write split.** Distinct label families have distinct writers:

- `orchestrator:*` — orchestrator-only, always.
- `state:*` and `worker:*` on an issue dispatched to this worker — the
  assigned worker updates its own (flip `state:🔨active` on pickup, flip
  `state:🛑blocked` on a user wait, strip labels on close-out).
- **Board Status** — derived from labels by reconcile (orchestrator or
  `fleet board sync`). Workers do not write Project field values directly.
- **Fleet work stack** — orchestrator-only; workers stamp
  `fleet heartbeat` at turn boundaries and otherwise leave the ledger alone.

On close-out, clean the seat: `cargo clean` in the worktree and
`scripts/nats-broker.sh down` if a broker was started.

## PR and merge authority

**Always open PRs as drafts.** Every `gh pr create` call includes `--draft`.

**Dispatch-authorized merges run autonomously.** When work arrives via
`/implement` on an agreed plan or epic, plan approval is the merge
authorization — the worker runs the full ready → review-bot loop →
`fleet merge` without a per-PR approval stop. Gates remain absolute: the CI
workflow green at head, the review bot's body read with every finding applied
or explicitly declined. Stop-at-green (surface for the user's merge decision)
is an explicit per-dispatch override, not the default for dispatched work.
