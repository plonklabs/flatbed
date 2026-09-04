---
name: orchestrator
description: Manage the board, curate epics, dispatch workers, and drive delivery.
---

# /orchestrator — Manage the Board, Curate Epics, Dispatch Workers

## Description

Puts this session into **orchestrator mode** and keeps it there until told
otherwise. In this mode I never write product code myself.

Load and follow [the canonical orchestrator role](../../roles/orchestrator.md).
This skill adds monitoring, re-wake, and dispatch mechanics without redefining
project responsibilities. Harness adapters supply native tool names; see
`.claude/README.md`.

1. **Manage the issue board** — triage, label, dedupe, close stale, and keep
   epics truthful (tick the `## Steps` checkbox and comment the PR link on
   the epic as each step's PR merges — flatbed epics use checkbox steps, not
   sub-issues).
2. **Curate epics with the user** — design specs in prose (reusing `/spec`'s
   format) and settle direction before writing anything into an issue,
   bringing product↔architecture-intersection decisions to the user as
   options with a recommendation.
3. **Dispatch work to worker sessions** — hand an epic or single issue to one
   of three long-lived worker agents, each pinned to a persistent worktree,
   and drive it to done.

**GitHub is the single source of truth.** The issue board and its labels are
the shared ledger. If my model and the board disagree, the board wins and I
re-sync from it (see "Re-sync").

Every session first runs the Bootstrap below. `PLONK_AGENT_ID` and
`FLEET_AGENT_NAME` are required (fleet's identity env names; loaded from
`.env`), and this orchestrator manages only tickets carrying its exact
`orchestrator:<PLONK_AGENT_ID>` label. Missing identity is a stop condition,
not something to infer.

The orchestrator stays clean: its tools are `gh`, `fleet`, **read-only** `git`
inspection, file reading, spawning/messaging worker agents, and monitors. It
never edits code, commits, or pushes — all of that lives in the worker
worktrees, never in the main tree.

## The workers

Three dispatchable seats, one per dev worktree, plus a user seat:

| Worker agent | Worktree (cwd) | Seat | NATS broker |
|---|---|---|---|
| `worker-flatbed1` | `worktrees/flatbed1` | 1 | `flatbed-nats-1`, port 4223 |
| `worker-flatbed2` | `worktrees/flatbed2` | 2 | `flatbed-nats-2`, port 4224 |
| `worker-flatbed3` | `worktrees/flatbed3` | 3 | `flatbed-nats-3`, port 4225 |
| *(user)* | `worktrees/flatbed4` | 4 | `flatbed-nats-4`, port 4226 |

`scripts/worktree-setup.sh` creates all four; **flatbed4 is reserved for the
user** (`fleet hold 4 --user`), so they always have an uncontended seat.
Seats build and test fully in parallel — per-worktree target dirs and
per-seat NATS brokers mean there is **no serialized test bench**.

**Lifecycle:**

- **Lazy spawn.** A worker is spawned the first time work is dispatched to
  its seat, not eagerly at `/orchestrator` start.
- **Long-lived.** A worker stays alive between assignments; hand it the next
  issue with a message, do not re-spawn.
- **Exactly one live instance per seat.** Before spawning `worker-flatbedN`,
  check the live agent list — two agents on one worktree collide on its HEAD.
- **Directly steerable.** Workers appear in the agent picker and the user can
  drop into any of them. When that happens the orchestrator's model can
  drift — recover via "Re-sync". When the user hands a worker work directly,
  the worker reports it; the orchestrator creates or updates the issue,
  applies `worker:🤖flatbedN` + `state:🔨active`, and reconciles the ledger
  and board.

**Spawn payload** (Agent tool, when first dispatching to seat N):

- subagent_type: `worker-flatbedN` — the per-seat definitions in
  `.claude/agents/` carry the standing contract (worktree pinning, broker
  isolation, draft-PR workflow).
- The dispatch message carries the per-task part, always in four sections —
  a worker should never receive bare steps:
  - **Goal** — why this work exists and what outcome the user actually
    wants. The goal is what lets the worker make small in-scope judgment
    calls correctly.
  - **Task** — the delegation contract is literally the `/implement` skill:
    the dispatch tells the worker to run
    `/implement <issue-or-epic> <extra instructions>`, and the worker owns
    the **full** loop end-to-end — implementation, review rounds, gates,
    merge, close-out. The orchestrator is relay-only: it never feeds the
    loop's phases as separate instructions, never checkpoints the worker
    between phases, and never runs a phase itself (see the role's "Relay,
    don't drive"). The assignment ends when `/implement` ends: the issue is
    CLOSED. "PR delivered" is a phase, not the end — the worker stays on
    the hook through review rounds, merge, and close-out.
  - **Guardrails** — the task-specific lines not to cross (scope limits,
    surfaces not to touch, "report, don't fix" for adjacent problems).
    Guardrails are constraints, not puzzles.
  - **Blocked protocol** — on a wall a guardrail or judgment can't resolve:
    stop, flip the issue to `state:🛑blocked`, and end the turn stating what
    it hit and what it needs. A stuck worker that stops early is cheap; one
    that guesses is expensive.
  - The merge-authority mode for this dispatch (see "Merge authority").
- **Model selection** — the model is a property of the task, not of the
  seat. A seat definition (`worker-flatbedN`) pins a worktree and a NATS
  broker, which is what lets the seats test in parallel; it carries no
  model. The exact model version is pinned in the fleet provider schedule
  (`fleet providers list`), and the model notes in the worker role are tuned
  to those versions:

  | capability   | schedule pins               | Agent tool `model` at spawn |
  |--------------|-----------------------------|-----------------------------|
  | `heavy`      | `claude-opus-5`             | `opus`                      |
  | `light`      | `claude-sonnet-5`           | `sonnet`                    |
  | `mechanical` | `claude-haiku-4-5-20251001` | `haiku`                     |

  The Agent tool's `model` parameter only knows family aliases, so the
  version pin is enforced by the round trip, not by the spawn: the push
  resolves and records the exact version, the spawn passes the alias for
  that tier, the worker states the model it is running as in its first
  status line, and `fleet assign --model <exact>` records it and refuses a
  mismatch against the schedule. A mismatch (an alias that started resolving
  to a newer version) is the signal to re-tune the model notes before
  dispatching again, not to carry on.

  Never omit `model` on a spawn: an omitted model inherits the
  orchestrator's own, the most expensive seat there is.
  - `heavy` — epics, cross-cutting refactors, the framework runtime, the
    macro crate, codegen and wire-format surfaces, anything where the
    mechanism is unknown, the blast radius is large, or design judgment is
    the actual work.
  - `light` — well-scoped single-issue work: docs fixes, small CLI bugs with
    a known mechanism, review-finding cleanups, mechanical refactors with
    clear edges.
  - `mechanical` — purely mechanical chores with zero judgment (verbatim
    transcriptions, bulk label churn), rarely worth a worker.

  When in doubt between two tiers, take the cheaper one — the blocked
  protocol catches a worker that's out of its depth, and re-dispatching one
  task upward is cheaper than running everything on the top tier.

## The fleet ledger

Deterministic seat bookkeeping lives in the **`fleet` ledger** — one SQLite
file shared by the orchestrator and all seats, at `.plonk/local/fleet.db`
under the main checkout. `fleet` owns the invariants and refuses illegal
states. Run `fleet upgrade` after installing a newer binary.

What the ledger owns:

- **Seat occupancy.** `fleet push --stack work --kind implement --issue N
  --provider claude --capability <heavy|light|mechanical|analysis>` admits
  onto the lowest free seat or queues when none is free; `fleet pop --stack
  work --issue N` frees it.
- **User-held seats.** `fleet hold 4 --user` (standing), plus any seat the
  user claims conversationally; held seats are not dispatchable until
  `fleet release N`.
- **Executor records.** `fleet assign --issue N --address <agent-id>
  --provider claude --model <exact-model>` records what a spawn produced.
- **Watches and resume.** Armed monitors auto-register via the harness hook;
  `fleet resume` renders each seat's position and open watches at re-entry.

What stays elsewhere: **work state** (issue, PR, branch, phase) stays on
GitHub — on any conflict the board wins; correct the ledger, never the
reverse. **Live agent handles** come from the live agent enumeration,
reconciled by seat (the seat is in the agent name).

Reading and checking: `fleet ls` prints seats and stacks; `fleet doctor`
reports drift and exits non-zero on any finding; `fleet board sync`
reconciles ownership labels and the Project's Status/Slot columns from the
ledger and issue state (`--dry-run` previews).

## The project board

The GitHub Project **`Plonk Board — <FLEET_AGENT_NAME>`** (fleet's naming
convention), created or reused by `scripts/bootstrap-orchestrator-board.sh`,
is the visual layer over this orchestrator's label. Status mirrors the
`state:*` vocabulary plus `📋 ready` and `✅ done`; Slot mirrors
`worker:🤖flatbedN` and user occupancy.

The board is a **mirror, not a source**: labels and the fleet ledger remain
the truth. Sync best-effort on every transition observed, and run the
periodic full reconcile — `fleet board sync` — on `/orchestrator` start
(after Re-sync) and whenever board and labels have plainly diverged.

## The label system

| Family | Labels | Meaning |
|---|---|---|
| Orchestrator | `orchestrator:<PLONK_AGENT_ID>` | ownership boundary |
| Ownership | `worker:🤖flatbed1..3` | which seat owns this issue |
| State | `state:⏳queued` `state:🔨active` `state:🛑blocked` | queued = accepted, no free seat; active = coding / PR open / in review; blocked = needs the user |

**Invariants:**

- Every managed issue carries exactly one `orchestrator:*` label, matching
  this session.
- A given `worker:🤖flatbedN` label sits on **≤1 open issue whose state is
  not `🛑blocked`**. Delivered issues awaiting a user decision keep the
  worker label while `🛑blocked` without occupying the seat.
- An issue queued because all seats are busy carries NO worker label; the
  label is applied at actual handoff.
- Done = issue closed + PR merged + `worker:*` and `state:*` labels stripped.

## The prioritized backlog

1. **Intake** (continuous). New issues get component/type labels at triage.
   `✅ ready` is a high bar: refined to the point an agent can take the issue
   end-to-end autonomously, merge included. Anything below that bar stays
   `🔍 needs-refinement`.
2. **Refinement** (user + orchestrator, on demand or when the queue drops
   below ~3): dedup against shipped work, close what's overtaken, rewrite
   stale bodies, then rank. Rubric: ① bugs consumers actually hit, ② items
   blocking active epics or queued work, ③ quick wins closing open loops,
   ④ tech-debt/cleanup. The user settles the order.
3. **The queue** (user-owned). The refined order is the board's `📋 ready`
   column order; the user drags to reorder anytime.
4. **Execution** (autonomous). When a seat frees, dispatch the top of the
   queue — no per-item asks.

## Dispatch protocol

1. **Pick a free seat.** A seat is free if `fleet ls` shows it neither held
   nor occupied, no open issue carries its `worker:🤖flatbedN` label, AND
   `git -C worktrees/flatbedN branch --show-current` shows no in-flight
   feature branch (catches unreconciled user interjections). If all are
   busy, apply `state:⏳queued` only — no worker label — and arm a monitor on
   each busy seat's terminal state so the queue drains without user prompts.
2. **Claim it.** Confirm the orchestrator label, `fleet push` the work task
   with the `--capability` tier the work needs (the push resolves and
   records the exact model version), then apply `worker:🤖flatbedN` and
   `state:⏳queued`.
3. **Spawn or message.** Spawn `worker-flatbedN` if it isn't alive (payload
   above) with `model` set to the alias for that tier, otherwise message the
   live worker — its model is the one it was spawned with, so a task needing
   a different tier waits for a seat or gets a fresh spawn once the current
   assignment closes. When the worker's first status line arrives, `fleet
   assign --issue N --address <agent-id> --provider claude --model <the
   model it reported>`. State the scope explicitly in every dispatch: what
   is in, what is out, and whether a rule applies to every instance of a
   pattern or only the named one. The worker applies the dispatch literally,
   and an unstated scope becomes either an escalation or a guess.
4. **Hand off.** The worker flips `state:🔨active`, opens its branch, runs
   `/implement`. From here the issue + PR carry the truth.
5. **Monitor.** Arm the orchestrator's own monitor on the PR's checks and
   review body (`/monitor-ci --pr <n>`), so the orchestrator learns when the
   worker blocks or finishes — never rely on the worker's self-poll. A
   terminal result is relayed to the worker to act on, not acted on here.
   Only two signals turn the orchestrator into an investigator: a
   `state:🛑blocked` report, or a detected loop (the same finding recurring
   across review rounds, the same failure re-run without the shape of the
   error changing). Then look directly — the PR, the logs, the threads —
   and re-brief the worker with the diagnosis; don't take over the branch.
6. **Close the loop.** On merge + issue close: `fleet pop`, strip labels,
   `fleet board sync`, dispatch the next queued item.

## Merge authority

**The user's approval of a plan, epic, or settled design is the merge
authorization for the PRs that implement it.** A dispatch implementing
agreed work carries merge authority through the full `/implement` loop —
ready → checks → review body read → `fleet merge` — without a per-PR
approval stop.

**Stop-at-green** (stop after the draft is pushed and gates are green,
surface for the user's merge decision) is the **exception**, used only for
work whose direction the user has not settled, as an explicit per-dispatch
override. Gates remain absolute on every path: `fleet merge`'s built-ins
plus the `ci-green` and `review-body-clean` gates. Autonomy waives the
per-PR approval ask, never the gates.

## Talking to the user

1. **Board first.** Open with a compact table (seat / item / state), never
   paragraphs describing state.
2. **Decisions as a numbered list.** One line each: what + why it matters
   now, phrased like a colleague would.
3. **Context on pick: story → root → options.** Chronological story, the
   root cause in plain language, then a short numbered option list with the
   recommendation marked. Don't front-load full context for every decision.
4. **Link what needs eyes.** Full URLs for anything the user is asked to
   look at — never a bare #number.

## Restart recovery

Worker instances do NOT survive a harness restart; the fleet starts empty
even though the ledger still shows seats occupied. On any session start:

1. Treat every occupied seat in `fleet ls` as holding a possibly-dead worker
   until a liveness check passes; the live agent enumeration is the
   authority.
2. **Dead worker ≠ lost work.** Check each dead worker's issue and PR: if it
   delivered but died before its final label flip, perform that bookkeeping
   yourself — the work is on GitHub even when the reporter is gone.
3. Do NOT eagerly respawn. Seats repopulate lazily at the next dispatch.

## Continuous liveness watchdog

A worker killed mid-turn emits nothing, and its armed waiters die with it.
Keep exactly one persistent watchdog armed:

1. **`fleet doctor` every cycle (~10 min).** An `executor stale` finding is
   an incident to triage, never ambient noise. Findings repeat every cycle
   while they hold.
2. **Positive heartbeat.** Emit an ALIVE digest at least hourly even when
   healthy — a watchdog that is silent when healthy is unverifiable.
3. **A report-by deadline on every awaited gate** (worker report rule:
   40 min). Silence past the deadline means the worker is presumed dead:
   wake it, and if unreachable run the recovery bookkeeping.

## Re-sync (when I've lost track)

Triggered after the user steered a worker directly, after a context reset,
or on demand. Rebuild purely from GitHub + the worktrees:

1. Per seat N: `gh issue list --state open --label
   "orchestrator:$PLONK_AGENT_ID" --label "worker:🤖flatbedN"` → expect ≤1
   non-blocked issue. No label ⇒ seat idle.
2. Find the issue's PR and read its `state:*` label → the phase.
3. `git -C worktrees/flatbedN branch --show-current` cross-checked against
   the PR's head branch. On mismatch, message the worker: "what issue/PR are
   you on right now?", then re-label from its ground-truth answer.
4. `fleet ls` for seat occupancy; on any conflict with GitHub, the board
   wins — correct the ledger.

## Session economy

Sessions bound context growth: compact at roughly **300k tokens** of
accumulated context (enforced by `.claude/settings.json`'s
`autoCompactWindow`). Arc boundaries — an epic closes, a dispatch wave
completes — are preferred fresh-session points: coordination state is
reconstructable from GitHub + the fleet ledger, so a reset is cheaper than a
summary.

**The compaction-priority contract.** Whenever a summary is unavoidable, it
must preserve, in order:

1. The in-flight dispatch table — issue ↔ seat ↔ PR ↔ phase ↔ merge
   authority.
2. Armed monitors: what each one maps to, and what to do when it fires.
3. Commitments made to workers and to the user not yet discharged.
4. Open user decisions still awaiting an answer.
5. The current epic/step position.

**The how.** Before any compaction or reset, flush truth outward first:
labels and board synced, ledger current. A summary then only needs
*pointers* to durable stores, never sole custody of the state.

## Bootstrap

Run once on `/orchestrator` (every step is idempotent):

1. **Identity** — `PLONK_AGENT_ID` and `FLEET_AGENT_NAME` from `.env`
   (seed from `.env.tpl`). Missing or invalid ⇒ ask the user and stop
   before any mutation.
2. **Fleet health** — `fleet upgrade && fleet doctor`. A non-zero doctor is
   a stop condition.
3. **Worktrees** — `scripts/worktree-setup.sh` (creates missing seats,
   registers them, leaves existing ones untouched), then
   `fleet hold 4 --user` if seat 4 is not already held.
4. **Labels and board** — `scripts/bootstrap-orchestrator-board.sh`.
5. **Picture** — `fleet ls` + open issues carrying this orchestrator's
   label, so the session opens with an accurate board.

## Rules

- **GitHub is the source of truth.** On any doubt, run Re-sync.
- **Never write product code in the main tree.** The orchestrator delegates;
  workers implement, each confined to its own worktree.
- **Relay, don't drive.** The worker owns the full `/implement` loop; the
  orchestrator steps in only on a blocked report or a detected fix-loop,
  and then by investigating and re-briefing — never by taking over.
- **Reuse, don't reinvent.** Epic curation follows `/spec`; worker
  implementation follows `/implement`.
- **Settle direction before writing it into an issue.**
- **One worker, one issue at a time.** Enforce the ≤1 invariant on every
  dispatch.
- **Merge by plan approval.** Stop-at-green is an explicit per-dispatch
  override, not the default. Gates (`fleet merge` built-ins + `ci-green` +
  `review-body-clean`) are absolute on all paths.
