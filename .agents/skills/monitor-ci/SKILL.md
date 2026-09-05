---
name: monitor-ci
description: Watch a pull request's checks and review body, verify main after a merge, and route terminal results to the owning worker.
---

# /monitor-ci — Watch PR Checks and Main Health

This is an orchestrator-owned workflow. It observes CI signals — a pull
request's checks and review-bot body, and the health of `main` after a
merge — then routes the terminal result to the responsible worker. It never
edits implementation files or fixes a failure itself. Harness adapters
supply native tool names for waiting and messaging; see `.claude/README.md`.

## Modes

### `/monitor-ci --pr <number>`

Watch one pull request through to a terminal check state.

1. Read the PR's current head SHA and pin it. Every observation is about
   that exact SHA; a result read against a superseded SHA is discarded, not
   acted on (the `stale` path).
2. Read the check rollup and the review bot's body. A required check a path
   filter legitimately skipped counts as passing; a non-terminal check keeps
   the result `running`.
3. Classify: `running` while any check is in flight; `failure` when a check
   concludes red; `success` when the CI workflow is green **and** the review
   body is clean; `review_findings` when checks are green but the bot body
   logs an unresolved finding. A green `review / review` check is never a
   clearance on its own — the body is read every time.
4. On a terminal result, re-wake the owning worker so it continues the PR
   lifecycle or remediates. `running`, `stale`, and `timeout` re-wake no
   one; they resume a fresh bounded check.

The monitor never merges. On `success` the woken worker runs `fleet merge <n>
--no-merge` — which re-evaluates not-draft, required checks at head,
`ci-green`, and `review-body-clean` against the current pinned SHA — and only
its exit 0 authorizes the admin merge that lands the commit.

### `/monitor-ci --main --after <merge-sha>`

After an orchestrated merge, verify `main` stayed healthy at that exact
merge point: identify the workflows the merge SHA triggered, read their
conclusions, classify `healthy` / `running` / `red`. A `red` result is only
actionable once its evidence is recorded — the failing run URL, the
workflow, the merge SHA, the last known green `main` reference. Attribution
stays unconfirmed: the newest merge is not blamed without a controlled
comparison against the last green state.

## Bounded waiting

Every wait is bounded to 15 minutes or less and reports its last observed
state on timeout. A wait delegated to a worker's own monitor gets a
report-by deadline on the orchestrator side (40 minutes): a worker session
killed mid-turn takes its armed monitors with it and emits nothing, so
silence past the deadline is a presumed-dead worker — wake it or run
recovery, never extend the benefit of the doubt without a new timer.

**On timeout, investigate — never shrug.** The next response must include:
(a) what the loop was watching, (b) the current state of that thing now,
(c) why the loop missed the terminal signal. "Stale, re-arming" without
those three pieces is not a complete response.

## Monitor script patterns

GitHub reads go over REST (`gh api repos/{owner}/{repo}/...`). Forbidden: `gh
pr view`, `gh pr checks`, `gh pr list`, `gh issue list`, `gh run view --json`,
`gh repo view`, any `--watch` — they wrap GraphQL, and a monitor is the one
caller that polls, so it is the one most able to trip the shared user's
GraphQL secondary rate limit.

Keep `--jq` filters to plain field selection plus the `//` default operator:

```bash
gh api repos/plonklabs/flatbed/actions/runs/<id> \
  --jq '.status + ":" + (.conclusion // "")'
```

Nested jq string interpolation inside bash-quoted strings silently emits
`parse error` to stderr on every poll, and stderr does not fire monitor
notifications — a monitor in that state spins silently to its deadline.

**GitHub returns `null` for `conclusion` on in-flight runs.** Gate on
`.status == "completed"` first, then test the conclusion; a gate that checks
`conclusion != "success"` without that order classifies every running
workflow as failed.

**Check-run `status` and `conclusion` are lowercase over REST**
(`completed`, `success`, `failure`); the UPPERCASE enums belong to the
GraphQL check-rollup shape, so a filter carried over from the porcelain
matches nothing and reads as "still running" forever.

## Review-check first-run flake

The `review / review` check can fail on a SHA's first run while CI is green.
On that pattern: confirm no new review body was posted for the SHA, then
`gh run rerun <id> --failed`. Do NOT rebase or force-push to retrigger —
that mints a new SHA and re-runs every check. A second failure on the same
SHA is real. Still read the rerun's review body before merging.

## Draft-skip race

Checks from a draft-state push can survive as `SKIPPED` after `gh pr ready`,
and GitHub counts `SKIPPED` as satisfied. **Never issue or arm a merge in
the same action as `gh pr ready`.** After flipping ready, confirm fresh runs
at the ready-state head; if checks sit `SKIPPED`, a no-op amend +
`git push --force-with-lease` produces a `synchronize` event that runs them.
`fleet merge` refuses a `SKIPPED` required check regardless — the two-step
discipline (ready, wait for fresh green, then merge) holds anyway.

## Failure handling

When `main` is red or a PR's checks fail, the orchestrator:

1. Records the failing run URL, the workflow, the SHA, and the last known
   green state.
2. Does not assume the newest merge caused the failure; compares evidence
   before attributing.
3. Reflects the blocker on the board and delegates diagnosis and remediation
   to a worker.
4. Re-runs monitoring against the repair's exact SHA.

**Read the first failing log deeply before spending reruns.** If the errors
carry a mechanism-shaped signature, diagnose instead of re-running.
Two-reds is for separating flake from persistent failure, not a substitute
for reading.

**Attribute failures before deflecting.** Before labeling a failure
"pre-existing on main", run `git log -1 --format='%h %an %ai %s' <path>` on
the culprit code. "Pre-existing" means genuinely old AND unrelated to recent
work — not "older than my branch."
