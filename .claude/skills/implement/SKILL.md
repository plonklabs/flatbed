# /implement — Execute an Agreed-Upon Stack of PRs Autonomously

## Description

Fires after a design discussion has settled an ordered list of PRs to ship. For each PR in order: implement → self-review and fix everything including nits → flip to ready → run `/review --auto` (bot review + apply-fixes loop) in parallel with cluster-side tests (e2e + smoke) → merge → move on. Pure-cleanup PRs skip the cluster tests; the last non-cleanup PR MUST be smoke-tested via the `plonk` CLI (never `kubectl`-mutating) after `make dev-reset`. Invoking this skill explicitly authorizes autonomous ready/merge for the listed PRs — it overrides the default [[feedback_draft_prs]] / [[feedback_explicit_merge_authorization]] rules for *this* invocation only.

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

2. **Otherwise**, re-state the ordered PR list inferred from the immediately preceding chat conversation.

3. **Print** the numbered list back to the user before doing anything else.

4. **If the list is ambiguous** (no clear order, zero items, scope missing from one or more entries), STOP and ask the user to restate the list. Never invent a PR not on the agreed list.

5. **State authorization once**: print one line confirming that the user's invocation of `/implement` authorizes autonomous flip-to-ready and squash-merge for the listed PRs only (this is the explicit override of [[feedback_draft_prs]] and [[feedback_explicit_merge_authorization]] for this run).

### Phase 1: Classify each PR

Annotate every PR with the following fields and print the classification table to the user before the loop starts:

- **`pure-cleanup: yes/no`** — yes if the entire PR is formatting / dead-code deletion / comment hygiene / dependency bumps with no behaviour change. Pure-cleanup PRs skip both e2e and smoke.
- **`smoke-test: required/skip`** — binary, no middle ground:
  - `required` if the PR changes the cluster's installed shape (operator kind, sidecar topology, namespace contents, RBAC, CRD wire format) — per [[feedback_smoke_test_scope]], "intermediate plumbing" is exactly where shape changes hide,
  - `required` if this is the last non-cleanup PR in the stack (mandatory floor — even a CLI-only wrapper is exercised end-to-end here),
  - `skip` for pure-cleanup PRs and for PRs that don't touch the cluster's post-install shape.
- **`e2e: required/skip`**:
  - `required` for any PR touching operator reconcilers, CRDs, CLI install/uninstall flows, worker contracts, or anything else `make e2e-worktree` exercises,
  - `skip` for pure-cleanup PRs and docs-only PRs.

  e2e is the automated test analog of the manual smoke. The two are independent: a PR can need both, either, or neither.
- **`branch-from: main | <prior-pr-branch>`** — default `main` (merge-first). Only stack if the user explicitly asked during the design discussion, or if a PR's diff cannot be built without the prior PR's code.

### Phase 2: Per-PR loop

For each PR in order:

#### 2a. Implement

1. Branch off `origin/main` (or the prior PR's branch when `branch-from` says so):
   ```bash
   git fetch origin main
   git switch -c feature/<descriptive-slug> origin/main
   ```
2. Implement only the changes scoped to this PR. Honour [[feedback_strict_migration_scope]] — don't fold in unrelated cleanups.
3. Run the gate suite locally:
   ```bash
   cargo fmt --all
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   cargo test -p <touched-package>
   ```
   All must be green before pushing.
4. Push and open a **draft** PR via `/pr`. Title and body follow the `/pr` skill's rules; link the epic when there is one.

Per [[feedback_draft_pr_is_checkpoint]] — push as soon as the code compiles and unit tests pass. e2e doesn't gate the draft push (it runs in Lane B later).

#### 2b. Self-review

1. Invoke `/review` against the draft PR (the review skill's Phase 1 — local self-review against the Quality checklist).
2. Apply **every** finding: blocking, quality, **and nits**. Autonomous mode does not skip nits.
3. Re-run `cargo fmt`, `cargo clippy -- -D warnings`, and `cargo test -p <pkg>` after the fix commits.
4. Self-review is "clean" when re-running it would surface only cosmetic phrasing already addressed and no behavioural issues remain. Apply [[feedback_review_thoroughness]] — fix the class across the whole change set; read whole files, not just diff hunks.

#### 2c. Flip to ready

```bash
gh pr ready <n>
```

This is the one place [[feedback_draft_prs]] is overridden — by invoking `/implement`, the user pre-authorized ready/merge for the agreed list (already stated in Phase 0 step 5).

#### 2d. Concurrent gates — bot review + cluster tests in parallel

Bot review (`/review --auto`) and cluster-side tests (e2e + smoke) run **concurrently** against the same HEAD to minimize wall-clock. Doing them serially wastes time when both take minutes.

**Lane A — `/review --auto`** (foreground in the conversation):
- Invoke `/review <pr#> --auto`. The autonomous review loop handles wait-for-bot → apply-fixes → re-trigger until the bot returns green (or surfaces a blocker).
- Each round of fixes Lane A pushes advances HEAD. When HEAD advances, Lane B must restart (see HEAD-pinning).
- If `/review --auto` exits non-green, STOP the outer loop and surface to the user. Never merge over a red bot review.

**Lane B — cluster test pipeline** (background, started right after 2c flips ready):
- The pipeline is **sequential internally** — e2e and smoke share the dev cluster, so they can't overlap each other — but the whole pipeline runs **in parallel with Lane A**.
- **B1: e2e** (when `e2e: required`). Run `make e2e-worktree` via Bash `run_in_background`. Watch with `Monitor` / `TaskOutput`. On green, proceed to B2. On red, see "Failure handling".
- **B2: smoke test** (when `smoke-test: required`):
  ```bash
  make dev-reset                                           # k3d cluster delete by slot-scoped name, then re-create + kubeconfig merge with --kubeconfig-switch-context
  make dev-load-images                                     # re-import operator + proxy + rocket + mesh-rocket from the host Docker daemon (cheap from layer cache) — required, else `plonk install` hits ErrImagePull with imagePullPolicy: IfNotPresent
  scripts/assert-kubectl-context.sh k3d-plonk-$PLONK_SLOT  # assert before any ad-hoc plonk install / uninstall — make dev-reset does NOT guard
  # then drive the vertical slice via the plonk CLI ONLY
  ```
  - The end-user invariant: every smoke interaction goes through `plonk`. `kubectl` is permitted **read-only** (`get`, `logs`, `describe`) for diagnosis — never `apply`, `delete`, `patch`, `rollout`, or any state mutation. Using `kubectl` to finish what `plonk` should do hides UX bugs the end user will hit.
  - Honour [[feedback_never_touch_plonk_production]] — `make dev-reset` is safe *by construction* (it deletes a slot-scoped k3d cluster by name, not by current context, then `--kubeconfig-switch-context` aims the active context at the recreated dev cluster), but it does NOT call `scripts/assert-kubectl-context.sh`. Ad-hoc `plonk install` / `plonk uninstall` invocations during smoke are NOT guarded; before each one, run `scripts/assert-kubectl-context.sh k3d-plonk-$PLONK_SLOT` explicitly.
  - Smoke scope: the vertical slice this PR completes. Honour [[feedback_smoke_test_scope]] — smoke-test any PR that changes the cluster's installed shape, not just the last PR in the stack — and [[feedback_smoke_test_differentiation]] — the two endpoint states must be visibly distinguishable via observable signals (different image refs, different startup-log fields) so the CLI can't lie about completion.
- **Skipping**: if both `e2e: skip` AND `smoke-test: skip` (typical for pure cleanup), Lane B is a no-op for this PR.

**HEAD-pinning**: tag every Lane B run with the commit SHA it started against. When Lane A pushes a fix commit:
- If Lane B is still running against the old SHA: kill it (`TaskStop`) and restart against the new SHA.
- If Lane B already finished against an old SHA: discard its result and restart against the new SHA.

The merge precondition is "bot green AND Lane B green against the **same** SHA as Lane A's final HEAD." Drift means re-run.

**Failure handling**:
- **Test flake** (transient infra, network blip, port collision): one automatic retry. A second failure is real.
- **Real test failure**: push a fix commit. That fix is just another push — Lane A picks it up via its bot re-review cycle; Lane B restarts (HEAD-pinning). Do NOT bypass with `kubectl`.
- **Design-level failure** (failure surfaces a design problem in the agreed PR, not a mechanical fix): STOP and surface to the user.

#### 2e. Merge

Precondition (all must hold at the same HEAD):
- Lane A green: `/review --auto` returned a green bot review.
- Lane B green (where required): e2e green, smoke green.
- Local gates green: unit tests + clippy `-D warnings` + fmt clean.

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
- For each PR, state which gates ran: bot review (always), e2e (yes/no + result), smoke (yes/no + result).
- If stopped on a blocker, name the PR, the failing step, and the surfaced output.

## Rules

- Never invent PRs not on the agreed list.
- Never skip nits in self-review under this skill — autonomous mode applies them all.
- Never use `kubectl` to mutate cluster state during a smoke test. `plonk` is the only mutator. `kubectl` read-only is fine for diagnosis.
- Pure-cleanup PRs (formatting / dead-code / dependency bumps with no behaviour change) skip both smoke and e2e — even when they are the last PR.
- `make dev-reset` is the only sanctioned cluster reset path; it is safe by construction (slot-scoped k3d delete by name + `--kubeconfig-switch-context`) but does NOT call `scripts/assert-kubectl-context.sh` — for ad-hoc `plonk install` / `plonk uninstall` during smoke, run the assertion script yourself first.
- `make e2e-worktree` is the e2e entry point. e2e + smoke form a single sequential Lane B pipeline (they share the dev cluster), started right after flipping ready, running concurrently with Lane A's bot-review loop. `make e2e-full` is for hermetic re-runs when worktree contamination is suspected.
- HEAD-pinning: every Lane B run is tagged with the SHA it started on. When Lane A pushes a fix, kill in-flight Lane B and restart against the new SHA. Merge requires both lanes green at the **same** SHA.
- Merge gates: `/review --auto` green AND e2e green (when required) AND smoke green (when required) AND fmt + clippy `-D warnings` + unit tests green at HEAD. All must hold — no exceptions.
- Test-flake policy: one automatic retry. A second failure is a real failure; do not paper over.
- Merge form: `gh pr merge <n> --squash --delete-branch --subject "<PR title>" --body "<PR description>"` — explicit `--subject` + `--body` prevents the default body (concatenated commit messages) from leaking any AI attribution that crept into a `/review --auto` fix commit.
- Run `/topr <pr#>` before re-opening review on a stacked PR.
- Honour [[feedback_functional_state]]: every merged PR leaves the system in a working state.
- Honour [[feedback_strict_migration_scope]]: per-PR scope stays mechanical; don't smuggle cleanups.
