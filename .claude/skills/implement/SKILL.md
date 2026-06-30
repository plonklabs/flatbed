# /implement — Execute an Agreed-Upon Stack of PRs Autonomously

## Description

Fires after a design discussion has settled an ordered list of PRs to ship. For each PR in order: implement → self-review and fix everything including nits → flip to ready → run `/review --auto` (bot review + apply-fixes loop) in parallel with an end-to-end smoke of the built artifact → merge → move on. Pure-cleanup PRs skip the smoke; the last non-cleanup PR MUST be smoke-tested by building and exercising the real artifact it completes. Invoking this skill explicitly authorizes autonomous ready/merge for the listed PRs — it overrides the default [[feedback_draft_prs]] / [[feedback_explicit_merge_authorization]] rules for *this* invocation only.

flatbed is a library + codegen repo (three crates — `flatbed`, `flatbed_macros`, `flatbed_build` — plus `examples/`), not a deployed service. There is no cluster: the gates are Cargo's, the codegen check, and running the affected artifact. The `flatc` pinned in `.flatc-version` must be on `PATH` for any build that triggers codegen.

## Arguments
- `$ARGUMENTS` — optional GitHub epic issue number (e.g. `/implement 614`). If given, the skill reads that epic's `## Steps` checklist (the format produced by `/spec`) as a starting hint. If both are present, the **chat conversation is the source of truth**; the epic is only consulted when the chat list is implicit.

## Instructions

When the user runs `/implement` or `/implement <epic>`, execute the following phases.

### Phase 0: Resolve the PR list

1. **If `$ARGUMENTS` was passed**, fetch the epic body:
   ```bash
   gh issue view <n> --json body --jq '.body'
   ```
   Extract unchecked `- [ ]` items under the `## Steps` section. These are the candidate PRs.

2. **Otherwise**, re-state the ordered PR list inferred from the immediately preceding chat conversation. A single agreed deliverable is a valid one-item list — treat it as one PR, don't force a split.

3. **Print** the numbered list back to the user before doing anything else.

4. **If the list is ambiguous** (no clear order, zero items, scope missing from one or more entries), STOP and ask the user to restate the list. Never invent a PR not on the agreed list. A clear single-PR deliverable is **not** ambiguous — proceed without asking.

5. **State authorization once**: print one line confirming that the user's invocation of `/implement` authorizes autonomous flip-to-ready and squash-merge for the listed PRs only (this is the explicit override of [[feedback_draft_prs]] and [[feedback_explicit_merge_authorization]] for this run). Having stated it, **do not re-ask for merge permission later** — that is exactly the re-prompting this skill exists to eliminate. The only thing that pauses the loop is a red gate or a design-level failure (see Failure handling), never caution about an irreversible step you were already authorized to take.

### Phase 1: Classify each PR

Annotate every PR with the following fields and print the classification table to the user before the loop starts:

- **`pure-cleanup: yes/no`** — yes if the entire PR is formatting / dead-code deletion / comment hygiene / dependency bumps with no behaviour change. Pure-cleanup PRs skip the smoke (the local gate suite still runs).
- **`smoke: required/skip`** — does a consumer-observable behaviour need to be exercised on a running artifact?
  - `required` if the PR changes runtime or wire behaviour someone can observe: route dispatch, request/response handling, content-type negotiation, the server boot/ready lifecycle, worker execution, telemetry/metrics output, the error wire format, **or codegen output** (the generated Rust a downstream crate compiles against).
  - `required` if this is the last non-cleanup PR in the stack (mandatory floor — even a thin wrapper is exercised end-to-end here).
  - `skip` for pure-cleanup PRs, docs-only PRs, and purely-internal refactors with no consumer-observable change.
- **`branch-from: main | <prior-pr-branch>`** — default `main` (merge-first). Only stack if the user explicitly asked during the design discussion, or if a PR's diff cannot be built without the prior PR's code.

The automated test suite is **not** a classification field — it always runs as part of the local gate suite (Phase 2a step 3) and is re-confirmed at merge. Smoke is the one gate that's per-PR optional, because running the artifact only makes sense when a consumer-observable behaviour changed.

### Phase 2: Per-PR loop

For each PR in order:

#### 2a. Implement

1. Branch off `origin/main` (or the prior PR's branch when `branch-from` says so):
   ```bash
   git fetch origin main
   git switch -c feature/<descriptive-slug> origin/main
   ```
2. Implement only the changes scoped to this PR. Honour [[feedback_strict_migration_scope]] — don't fold in unrelated cleanups.
3. Run the local gate suite (`flatc` on `PATH`). This is the always-run automated baseline, regardless of the `smoke` classification:
   ```bash
   cargo fmt --all
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   cargo test --workspace          # add --all-features when feature-gated code changed
   bash scripts/check-generated.sh # when schemas or codegen changed
   ```
   For changes to a standalone package outside the workspace (e.g. an `examples/<name>/` crate with its own `[workspace]`), run `cargo fmt`/`clippy -D warnings`/`build` inside that directory too — the workspace gates don't reach it.
   All must be green before pushing.
4. Push and open a **draft** PR via `/pr`. Title and body follow the `/pr` skill's rules; link the epic when there is one.

Per [[feedback_draft_pr_is_checkpoint]] — push as soon as the code compiles and the local gates pass. Smoke doesn't gate the draft push (it runs in Lane B later).

#### 2b. Self-review

1. Invoke `/review` against the draft PR (the review skill's Phase 1 — local self-review against the Quality checklist). For a docs/examples-only PR with no framework-logic change, a focused read of the whole change set for correctness and consistency is a sufficient self-review — don't spin up an adversarial pass that has nothing to bite on.
2. Apply **every** finding: blocking, quality, **and nits**. Autonomous mode does not skip nits.
3. Re-run the local gate suite after the fix commits.
4. Self-review is "clean" when re-running it would surface only cosmetic phrasing already addressed and no behavioural issues remain. Apply [[feedback_review_thoroughness]] — fix the class across the whole change set; read whole files, not just diff hunks.

#### 2c. Flip to ready

```bash
gh pr ready <n>
```

This is the one place [[feedback_draft_prs]] is overridden — by invoking `/implement`, the user pre-authorized ready/merge for the agreed list (already stated in Phase 0 step 5).

#### 2d. Concurrent gates — bot review + artifact smoke in parallel

Bot review (`/review --auto`) and the smoke (Lane B) run **concurrently** against the same HEAD to minimize wall-clock. Doing them serially wastes time when both take minutes.

**Lane A — `/review --auto`** (foreground in the conversation):
- Invoke `/review <pr#> --auto`. The autonomous review loop handles wait-for-bot → apply-fixes → re-trigger until the bot returns green (or surfaces a blocker).
- Each round of fixes Lane A pushes advances HEAD. When HEAD advances, Lane B must restart (see HEAD-pinning).
- If `/review --auto` exits non-green, STOP the outer loop and surface to the user. Never merge over a red bot review.

**Lane B — artifact smoke** (background, started right after 2c flips ready, when `smoke: required`). **Build and exercise the real artifact end-to-end**, matched to what the PR changed. There is no cluster; the artifact is a process you run on this host.
- **Server / runtime change** (route dispatch, request/response, content negotiation, boot/ready lifecycle, workers, telemetry, error format): run a service that exercises it — the relevant `examples/` service, or a small throwaway binary — and `curl` the affected endpoints, asserting the **observable** result (response body, status, `/healthz` ↔ `/readyz` transition, `/metrics` counter value, a worker's log line). Where the change touches content negotiation, hit it with **both** `application/json` and `application/x-flatbuffers`.
- **Codegen / macro change** (`flatbed_build`, `#[route]` / `#[worker]` output): run `flatbed generate` on a schema (or build an example whose `build.rs` drives codegen), then compile and run the result so the generated code is actually executed, not just emitted.
- **`examples/` change**: `docker compose up --build` for each affected example and `curl` its endpoints, including any sidecar the compose file starts (Swagger UI, Prometheus). Examples bind the same host port, so smoke them **sequentially** (`up` → assert → `down`).
- The smoke must surface a signal that **distinguishes done from not-done** ([[feedback_smoke_test_differentiation]]) — a real response body, a metric value, a log line — not merely "it compiled." A green `cargo build` is necessary but is never the smoke result on its own.
- **Skipping**: when `smoke: skip` (pure-cleanup or docs-only), Lane B is a no-op for this PR — the local gate suite from 2a is the whole automated story.

**HEAD-pinning**: tag every Lane B run with the commit SHA it started against. When Lane A pushes a fix commit:
- If Lane B is still running against the old SHA: kill it (`TaskStop`) and restart against the new SHA.
- If Lane B already finished against an old SHA: discard its result and restart against the new SHA.

The merge precondition is "bot green AND Lane B green against the **same** SHA as Lane A's final HEAD." Drift means re-run.

**Failure handling**:
- **Test flake** (transient infra, network blip, port collision, a slow container not yet ready): one automatic retry. A second failure is real.
- **Real failure**: push a fix commit. That fix is just another push — Lane A picks it up via its bot re-review cycle; Lane B restarts (HEAD-pinning).
- **Design-level failure** (the failure surfaces a problem in the agreed PR's design, not a mechanical fix): STOP and surface to the user.

#### 2e. Merge

Precondition (all must hold at the same HEAD):
- Lane A green: `/review --auto` returned a green bot review.
- Lane B green (where required): smoke green.
- Local gate suite green: `cargo fmt --check` clean + clippy `-D warnings` + `cargo test --workspace` (+ `--all-features` when feature code changed) + `check-generated.sh` (when codegen touched).

Squash-merge per project policy. Pin the merge-commit subject to the PR title and the body to the PR description, instead of letting GitHub concatenate every in-PR commit message into the body — that default form lets any AI-attribution slip introduced by `/review --auto`'s fix commits leak into the squash commit on `main` (the per-commit messages are out of sight by then). The PR description is what reviewers actually saw; it's the right canonical record:

```bash
TITLE=$(gh pr view <n> --json title --jq '.title')
BODY=$(gh pr view <n> --json body --jq '.body')
gh pr merge <n> --squash --delete-branch --subject "$TITLE" --body "$BODY"
git fetch origin main && git log -1 origin/main   # verify the merge landed
```

#### 2f. Prep next PR

- If next PR's `branch-from: main`: `git fetch origin main` and start 2a fresh from main.
- If next PR is stacked on this one: run `/topr <next-pr#>` to drop the just-squashed commits and rebase onto fresh main.

### Phase 3: Final report

When the loop completes (all PRs merged) OR stops on a blocker:

- Print the merged PR list with URLs and commit SHAs.
- For each PR, state which gates ran: bot review (always), local gate suite (always), smoke (yes/no — and when yes, what was exercised and the observed signal).
- If stopped on a blocker, name the PR, the failing step, and the surfaced output.

## Rules

- Never invent PRs not on the agreed list. A clear single-PR deliverable is a valid list — don't force a split and don't re-ask to confirm it.
- Authorization is stated once (Phase 0 step 5) and stands for the whole run. Do **not** pause to re-confirm flip-to-ready or merge — that re-prompting is the exact failure mode this skill removes. Only a red gate or a design-level failure stops the loop.
- Never skip nits in self-review under this skill — autonomous mode applies them all.
- `flatc` (version pinned in `.flatc-version`) must be on `PATH` for any build that triggers codegen; version drift produces byte-level diffs that make `check-generated.sh` report stale.
- Smoke means running the artifact and observing a distinguishing signal — never substitute "it compiled" for it. Build + run the affected example/service/CLI and assert real output.
- Standalone packages outside the workspace (e.g. `examples/<name>/` with its own `[workspace]` table) are not reached by `cargo *--workspace*` gates — fmt/clippy/build them in their own directory.
- Lane B (smoke) starts right after flipping ready and runs concurrently with Lane A's bot-review loop.
- HEAD-pinning: every Lane B run is tagged with the SHA it started on. When Lane A pushes a fix, kill in-flight Lane B and restart against the new SHA. Merge requires both lanes green at the **same** SHA.
- Merge gates: `/review --auto` green AND smoke green (when required) AND the local gate suite (fmt clean + clippy `-D warnings` + `cargo test --workspace`, plus `check-generated.sh` when codegen changed) green at HEAD. All must hold — no exceptions.
- Test-flake policy: one automatic retry. A second failure is a real failure; do not paper over.
- Merge form: `gh pr merge <n> --squash --delete-branch --subject "<PR title>" --body "<PR description>"` — explicit `--subject` + `--body` prevents the default body (concatenated commit messages) from leaking any AI attribution that crept into a `/review --auto` fix commit.
- Run `/topr <pr#>` before re-opening review on a stacked PR.
- Honour [[feedback_functional_state]]: every merged PR leaves the system in a working state.
- Honour [[feedback_strict_migration_scope]]: per-PR scope stays mechanical; don't smuggle cleanups.
