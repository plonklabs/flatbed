# /review — Self-review a draft PR or address bot review comments

## Description

Two-mode workflow keyed off PR state, with an opt-in autonomous loop:

- **Draft PR** → the assistant performs a local self-review against the [Quality checklist](#quality-checklist) and prints findings in the chat. Nothing is posted to GitHub. The point is to surface issues *before* flipping to ready, so the developer can fix them before the `claude[bot]` reviewer (configured in `.github/workflows/claude-review.yml`) sees them.
- **Ready-for-review PR** → the assistant waits for the bot's review run for the current HEAD commit to finish, then fetches its unresolved inline threads + top-level comments and runs a holistic fix loop. The fix loop reads each affected file in full (not just diff hunks) and applies the [Quality checklist](#quality-checklist) to catch siblings of every flagged issue. By default, Phase 2c asks the user to fix/decline each thread.
- **Autonomous mode** (`/review <pr-number> --auto`) → same as ready-for-review mode but skips Phase 2c (every bot finding is treated as fix-bound) and chains rounds: after Phase 2f posts the top-level summary, re-arms the wait loop on the new HEAD and goes back to Phase 2a. Loops until the bot returns a green review, the workflow run fails or times out, or the user interrupts. **Never marks the PR ready, never merges.**

## Instructions

When the user runs `/review`, `/review <pr-number>`, or `/review <pr-number> --auto`, follow this workflow:

### Phase 0: Detect PR state and parse flags

```bash
PR_NUMBER=${1:-$(gh pr view --json number --jq '.number')}
AUTONOMOUS=false
ROUND=1   # incremented at the end of each pass (Phase 2g), after posting the round summary
for arg in "$@"; do
    case "$arg" in
        --auto|--autonomous) AUTONOMOUS=true ;;
    esac
done
gh pr view "$PR_NUMBER" --json isDraft,state,headRefOid,headRefName
```

- If `isDraft == true` → **Phase 1** (draft mode). `--auto` is a no-op in draft mode (the bot doesn't run on drafts); proceed with Phase 1 as normal and tell the user that `--auto` takes effect once they flip to ready.
- If `isDraft == false && state == "OPEN"` → **Phase 2** (ready mode). If `AUTONOMOUS=true`, Phase 2c is skipped and Phase 2g re-arms the loop after each round.
- If `state` is `MERGED` or `CLOSED`, ask the user whether they want to review historical comments anyway, then go to Phase 2 (skipping the wait sub-phase). `--auto` has no effect on merged/closed PRs since there's nothing to re-trigger.

### Phase 1: Draft mode — local self-review

1. **Read the diff and the commit list via the GitHub API** (works whether or not the PR branch is checked out locally):
   ```bash
   gh pr diff "$PR_NUMBER"
   gh pr view "$PR_NUMBER" --json commits \
       --jq '.commits[] | "\(.oid[0:7]) \(.messageHeadline)"'
   ```

2. **Read the FULL files involved**, not just the diff hunks. The "read just the diff" pattern produces pinpoint comments and misses class-level issues. For each file the diff touches, open it end to end.

3. **Walk the [Quality checklist](#quality-checklist)** against the changes. For every item, ask: does this PR introduce or worsen this category of issue?

4. **Print findings in the chat**, grouped by file and severity:
   - **Blocking** — real correctness bugs, security issues, broken tests
   - **Quality** — CLAUDE.md violations, duplication, doc/code drift, missing edge-case coverage
   - **Nits** — style, naming, comment phrasing

   For each finding: file path, line range, the issue, and a concrete suggestion. Use the same tone as a thorough human reviewer.

5. **Do not post anything to GitHub.** No `gh pr comment`, no `gh api ... /comments`. The output is for the developer to read in chat and act on locally before flipping the PR to ready. Flipping to ready will trigger the actual `claude[bot]` review run via the `synchronize` / `ready_for_review` workflow event.

### Phase 2: Ready-for-review mode — wait for bot, then fix

#### Phase 2a: Wait for the bot review to complete

```bash
HEAD_SHA=$(gh pr view "$PR_NUMBER" --json headRefOid --jq '.headRefOid')
BRANCH=$(gh pr view "$PR_NUMBER" --json headRefName --jq '.headRefName')

WAIT_DEADLINE=$(($(date +%s) + 1800))   # 30 minutes
TRIGGER_DEADLINE=$(($(date +%s) + 60))  # 60 s for the workflow to appear

while :; do
    NOW=$(date +%s)
    if [ "$NOW" -gt "$WAIT_DEADLINE" ]; then
        echo "Bot review did not finish within 30 minutes — surfacing for user." >&2
        break
    fi

    # Pull status + conclusion for the run matching HEAD_SHA. The
    # `--jq` filter emits a single colon-joined string ("status:concl")
    # so we don't depend on multi-line JSON parsing — `head -1` on
    # `gh ... --jq '.[] | select(...)'` would only capture the
    # opening `{` of the first object.
    RESULT=$(gh run list --workflow=claude-review.yml \
        --branch "$BRANCH" --limit 10 \
        --json status,conclusion,headSha \
        --jq "first(.[] | select(.headSha == \"$HEAD_SHA\"))
              | \"\(.status):\(.conclusion // \"\")\"")

    case "$RESULT" in
        "")
            # No matching run yet.
            if [ "$NOW" -gt "$TRIGGER_DEADLINE" ]; then
                echo "No claude-review run found for $HEAD_SHA — workflow may have been skipped." >&2
                break
            fi
            sleep 10
            ;;
        "in_progress:"*|"queued:"*|"waiting:"*|"requested:"*|"pending:"*)
            sleep 15
            ;;
        "completed:success")
            break
            ;;
        "completed:"*)
            # Run completed but didn't succeed (timed out, errored,
            # cancelled by the workflow's own concurrency policy).
            # Surface to the user — don't silently treat as done.
            CONCL="${RESULT#completed:}"
            echo "claude-review run completed with conclusion=${CONCL:-<empty>}; surfacing for user." >&2
            break
            ;;
        *)
            echo "Unexpected workflow status: $RESULT — surfacing for user." >&2
            break
            ;;
    esac
done
```

If the loop breaks for any reason other than `completed:success`, ask the user how to proceed — don't silently continue as if the bot finished its review.

**Use the `Monitor` tool instead of inline `sleep` polling when one is available.** The polling shape above is the reference behaviour; in practice the harness's `Monitor` tool (with a script that emits one stdout line per status change and exits on a terminal state) gives a single notification when the run completes without keeping the conversation paused. Either form is fine; the Monitor form is preferred in autonomous mode because it lets the assistant keep working between rounds.

#### Phase 2b: Fetch unresolved threads

**Fetch UNRESOLVED inline threads via GraphQL.** Filtering REST `/comments` by timestamp or commit anchor is fragile — pagination silently truncates the response after 30 entries (a 33-comment PR's tail vanishes), and a clock cutoff that lands after a finding's `created_at` makes the round look green when it wasn't. Phase 2d's resolve-after-reply step (below) marks every addressed thread as resolved in GitHub's first-class state machine, so the round's actual working set is simply "all unresolved bot threads" — no filter heuristics, no pagination drift. Use GraphQL because thread resolution state isn't on the REST `/comments` shape:

```bash
gh api graphql --paginate -f query='
  query($owner: String!, $repo: String!, $pr: Int!, $endCursor: String) {
    repository(owner: $owner, name: $repo) {
      pullRequest(number: $pr) {
        reviewThreads(first: 100, after: $endCursor) {
          pageInfo { hasNextPage endCursor }
          nodes {
            id          # GraphQL node id — pass this to resolveReviewThread later
            isResolved
            path
            comments(first: 1) {
              nodes {
                databaseId   # the REST API "id" — needed for /replies
                author { __typename login }
                body
                createdAt
                line
              }
            }
          }
        }
      }
    }
  }' \
  -f owner=plonklabs -f repo=plonk -F pr="$PR_NUMBER" \
  --jq '.data.repository.pullRequest.reviewThreads.nodes[]
        | select(.isResolved == false
                 and .comments.nodes[0].author.__typename == "Bot"
                 and .comments.nodes[0].author.login == "claude")'
```

That streams each unresolved bot thread as one object with the shape `{id, isResolved, path, comments: {nodes: [{databaseId, author, body, createdAt, line}]}}` — exactly the set the loop needs to act on this round. Four pitfalls worth pinning so the next implementer doesn't have to discover them from a runtime error or a silently-undercounted batch:

- The thread's own identifier is `id` (the GraphQL node id), not `threadId`. Pass it as-is to `resolveReviewThread` below.
- `comments` on each thread is a connection object — access the first comment via `comments.nodes[0]`, not `comments[0]`.
- `gh api graphql --paginate` chases `pageInfo.endCursor` automatically as long as the query exposes `pageInfo { hasNextPage endCursor }` and accepts a `$endCursor: String` variable. Without it, `first: 100` silently caps the result and reintroduces the same under-count class of bug REST's 30-default produces — a PR with 101 review threads loses its tail.
- `--paginate` runs the `--jq` filter **per page** — a `map(select(...))` filter outputs one JSON array per page (multiple separate JSON documents on stdout), not a single merged array. A consumer that pipes the result into something expecting a single array silently drops every page after the first. The block above uses the streaming form `.nodes[] | select(...)` instead, which emits one independent JSON object per match — that's the actual NDJSON shape (newline-delimited self-contained values), which any line-by-line reader or `jq -s '.'` step collects into a list. The alternative is the array-map filter with `| jq -s 'add'` appended.

And the author filter combines two signals — `author.__typename == "Bot"` discriminates bots from human users (GraphQL exposes the bot's login as bare `claude`, with no `[bot]` suffix, so the REST-style `login == "claude[bot]"` string filter would silently mismatch every bot thread), and `author.login == "claude"` narrows to this project's reviewer specifically. Without the login check, dependabot, github-actions, or any other bot that ever leaves an inline thread enters the resolve/reply loop's working set — the same silent-inclusion class these filters eliminate elsewhere.

Top-level review summaries still come from REST (no resolution state on those). The REST API renders GitHub Apps with a `<name>[bot]` login, so the filter at this one site is `claude[bot]` — distinct from the GraphQL filter above, where the same bot appears as bare `claude`:

```bash
gh api "repos/plonklabs/plonk/issues/$PR_NUMBER/comments" \
    --jq '.[] | select(.user.login == "claude[bot]") |
          {id, body, created_at}'
```

If the GraphQL query returns zero unresolved bot threads (and the latest bot run completed without a new top-level summary that claims a blocker), the bot's findings are all addressed — report and stop.

#### Phase 2c: Triage with the user

**If `AUTONOMOUS=true`, skip this phase entirely.** Invocation with `--auto` is the explicit user direction to treat every bot finding as fix-bound — proceed straight to Phase 2d.

Otherwise: before applying any fix, present the bot's threads to the user grouped by file, with a one-line summary of each. For each thread (or batch of related threads on the same file), ask the user whether to **fix** or **decline**. Don't proceed to Phase 2d until the user has chosen for every thread.

This step exists because the Rules below require explicit user direction before fixing or skipping a bot thread. Skipping this step (other than via `--auto`) and going straight to fixes denies the user that decision point.

#### Phase 2d: Apply fixes holistically

For each batch of fix-bound threads on the **same file**:

1. **Read the FULL file** (not just the lines around the thread's anchor). The reactive line-fix loop is the failure mode that drives multi-round review iteration: every round introduces a new pinpoint patch that leaves adjacent issues uncovered.
2. **Read the immediate neighbours** the thread alludes to. If the thread is about a constant duplication, also open the file holding the canonical constant. If it's about a doc/code drift, also open the docs that reference the changed value.
3. **Walk the [Quality checklist](#quality-checklist)** and fix the thread's finding AND any adjacent issues that fall under the same category. The sweep must span **every file in the change set** (`git diff origin/main..HEAD --name-only`), not just the file the thread is anchored to. A "constant duplication" finding on one literal should trigger a sweep for that pattern in every changed file; a "hardcoded value" finding, a "forward-looking phrase" finding, an "error type quality" finding — all the same. Within-file sweeps miss cross-file siblings, which is the failure mode `feedback_class_level_fixes.md` captures and which has shipped regressions on this codebase.
4. **Stage and commit** the comprehensive fix as **one commit per file or per logical unit** — not one commit per inline thread. The commit message should describe the comprehensive change, not just the thread that triggered it.
5. **Run the relevant verifications before pushing:**
   - `cargo fmt --all` (apply formatting; AI-written code often needs normalisation, and `cargo fmt --all -- --check` would just fail rather than fix)
   - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
   - `cargo test -p <affected-package> --bins` (or workspace tests if the change is wide)
   - For changes touching the operator runtime: `make e2e-full` per CLAUDE.md

   **If any check fails, stop. Surface the output to the user, do not push, and do not paper over the failure with hand-edits that don't address the underlying cause.** A failed `cargo test` means the fix introduced a regression; a failed `clippy` means the fix introduced new style debt; both are blockers for the SHA-in-reply flow because the SHA being referenced wouldn't pass CI on `main`. Iterate on the fix until verifications pass, then push.
6. **Push.** Then reply on each addressed inline thread with the commit SHA:

   ```bash
   gh api -X POST \
       "repos/plonklabs/plonk/pulls/$PR_NUMBER/comments/<comment-id>/replies" \
       -f body='Fixed in <commit-sha>. <one-sentence what changed and why it covers more than the line>'
   ```

7. **Resolve the thread.** Immediately after the reply, mark the thread resolved via GraphQL — this is what makes the next round's "unresolved threads" query produce exactly the new findings:

   ```bash
   gh api graphql -f query='
     mutation($threadId: ID!) {
       resolveReviewThread(input: { threadId: $threadId }) {
         thread { id isResolved }
       }
     }' -f threadId="<thread-graphql-id>"
   ```

   `<thread-graphql-id>` is the `id` field on the GraphQL `reviewThreads.nodes[*]` shape — NOT the REST comment id (`comments.nodes[0].databaseId` from the Phase 2b output, used as `<comment-id>` in step 6's `/replies` call). Both are returned together by the Phase 2b fetch query so the pairing is one lookup. If the resolve mutation fails (rare — a stale node id or a race against another resolver — `gh api graphql` exits non-zero and the JSON body contains an `errors` array), surface the failure to the user with the affected thread's node id and abort the current round — do not continue resolving or replying on other threads. **In autonomous mode this exits the loop (same close-out flow as exit condition 2, bot run failed) — the fix is already pushed to the remote but the thread stays unresolved, so re-entering Phase 2b would re-fetch it as a phantom "still open" finding and the loop would mis-process it as a false recurrence on the next round.** The user resolves the surfaced thread manually before re-invoking `/review --auto`.

#### Phase 2e: Decline path

For each thread the user marked as **decline** in Phase 2c:

1. Ask for the reasoning (or draft a response and confirm with the user before posting).
2. Reply on the thread with the rationale (no commit SHA needed):

   ```bash
   gh api -X POST \
       "repos/plonklabs/plonk/pulls/$PR_NUMBER/comments/<comment-id>/replies" \
       -f body='<reasoning>'
   ```

3. Resolve the thread via the same GraphQL mutation as Phase 2d step 7. If the resolve mutation fails, apply the same abort-and-exit behavior as Phase 2d step 7: surface the affected thread's node id to the user, abort the current round, and in autonomous mode exit the loop — the phantom-recurrence risk is identical, and `--auto` skips Phase 2c so the unresolved declined thread would be re-fetched and mis-processed as a new fix-bound finding rather than an already-resolved decline. Decline-with-reasoning counts as addressed for the loop's purposes — the rationale is on the thread, the reviewer can see it, and re-opening the thread is the bot's job if it disagrees on the next round.

#### Phase 2f: Post the top-level summary

Run *after* both Phase 2d and Phase 2e have completed — the summary references state from both (fixed threads, declined threads) and posting it earlier means claiming a state that hasn't yet been produced.

```bash
gh pr comment "$PR_NUMBER" --body "<summary>"
```

The summary should cover:
- Reviewer-flagged issues fixed (with commit SHAs)
- Self-flagged issues fixed in the same pass (the holistic delta)
- Threads declined (with reasoning)

In autonomous mode, title each round's summary `## Round N — addressed` (track `N` across rounds in the loop state) so the user can follow progress in the PR conversation tab without scrolling.

#### Phase 2g: Autonomous loop (opt-in, `--auto` only)

If `AUTONOMOUS=true`, the push at the end of Phase 2d re-triggers the bot workflow on the new HEAD. After Phase 2f posts the round's top-level summary, loop back to Phase 2a with the new HEAD SHA:

```bash
NEW_HEAD=$(git rev-parse HEAD)
ROUND=$((ROUND + 1))
# Re-arm Monitor on NEW_HEAD using the same status-poll script as Phase 2a.
# Each round produces one stdout event on terminal status; the conversation
# stays free to do other work between rounds.
```

**Exit the loop** when any of the following hold. On each exit, post a final close-out summary that names the exit reason and the round count.

1. **Green review.** The Phase 2b GraphQL query returns zero unresolved bot threads AND the top-level summary doesn't claim a blocker. Positive signals: "Ready to merge", "No blocking concerns", "no issues found", bare "clean" / "LGTM" (not followed by a qualifier). Negative signals: an unresolved bot thread, "Blocking:", a numbered fix list, "must change", "regression", "consider", "optional", "minor nit". **Qualifier rule:** any positive signal followed by "but", "though", "however", "except", "consider", or a suggestion is a negative signal — the qualifier moves the verdict from green to ambiguous, and ambiguous reverts to another round per the Rules section. "LGTM but consider X" / "clean overall but Y" are negatives, not positives, so substring matching on the bare phrase must not exit. When in doubt, ask the user before exiting — false-positive green exit silently leaves a finding unaddressed; false-positive red just runs another round.
2. **Bot run failed.** `completed:failure`, `completed:cancelled`, `completed:timed_out`, or any non-success terminal status. Surface to the user; the loop exits. Don't auto-retry the workflow.
3. **Monitor timeout.** 30 minutes elapsed without the bot run reaching a terminal state. Surface and exit.
4. **User interrupts.** Any out-of-band user message during the loop. Pause the loop, address the user, and ask whether to resume.
5. **Recurrence.** If the same file region (file path + 5-line window around the anchor line) is flagged in two consecutive rounds despite an applied fix, break the loop. The autonomous mode's "every finding is fix-bound" stance assumes the assistant can satisfy the bot in finite rounds; a recurrence means either the fix misreads the finding or the bot's expectation is unattainable. Post a summary of what was tried on the recurring region and ask the user whether to decline the finding (with reasoning), take a different approach, or accept the current state. The `cargo test` / clippy guards catch fixes that break CI but not fixes that pass CI while still mismatching the bot's intent — recurrence is the complementary signal for that case.

6. **Speculative-fix saturation.** If three consecutive rounds produce **real correctness findings** (not nits) AND the fixes you applied to earlier rounds were based on guessed semantics rather than docs or repo precedent, break out of speculation mode for one round. Do not push another patch. Instead: re-audit the **entire diff** against the official docs of whatever semantic system is at play (GitHub Actions expressions, language coercion, the foreign API's spec), walk every input case through documented rules, and write down the expected result for each before editing. Ship one careful commit that covers everything the audit surfaces, *with the comparison reasoning in the commit message so a future re-reader can verify it*. PR #377 ran 4 rounds before this — a doc-grounded re-audit on round 5 found two bugs the loop had been preserving (a wrong K8s resource type and a broken expression coercion) and closed the loop clean. The signal "the bot keeps finding real bugs in semantics-adjacent fixes" means you don't yet know the semantics; reading the spec is faster than guessing again. See Quality checklist item 15 and [[verify-dont-guess-semantics]].

**Diff-vs-main drift check at the top of every round.** Before fetching unresolved threads, run `git fetch origin main && git log --oneline HEAD..origin/main` — if main has advanced, rebase the branch onto current main, run `git push --force-with-lease`, then `HEAD_SHA=$(git rev-parse HEAD)` and **re-enter Phase 2a** (not Phase 2b) — the push re-triggers the bot workflow on the new HEAD and Phase 2b must not run until that new run completes, or it would fetch threads produced against the pre-rebase HEAD. A long-running autonomous loop is the exact scenario where a stale base silently reverts merges that landed on main mid-loop (per [[feedback_rebase_against_latest_main]]).

**If the rebase produces conflicts, break the loop immediately.** Do not push. Surface the conflict to the user (show `git status` and the conflicting file list) and ask whether to (a) abort the rebase (`git rebase --abort`) and continue from the pre-rebase HEAD, (b) resolve conflicts manually before resuming, or (c) abandon the round. Same close-out flow as exit condition 2 (bot run failed): automation hit a hard stop, surface, don't auto-retry.

**Constraints that hold across the loop:**

- **Never auto-mark-ready, never auto-merge.** This rule survives autonomous mode unchanged. The loop fixes findings; the user decides when to merge.
- **Stop on test or clippy failure.** Per Phase 2d step 5 — a failed verification breaks the loop. Surface the failure to the user instead of pushing a broken SHA into the next round.
- **Skip Phase 2c each round.** No per-thread triage; every bot finding is fix-bound.
- **Class-level sweep applies every round.** The bot's findings narrow over time but the diff grows; rerun the full Quality-checklist sweep across every changed file each round, not just the file the new threads anchor to.

## Quality checklist

The substantive content the assistant works through in both modes. Each item is a category of issue that's bitten this codebase before — see `feedback_*.md` memories for the recurring patterns.

1. **Slice / PR / issue references in source comments.** Saved feedback: "Don't reference slice/PR/issue numbers in code comments; they rot. Planning context belongs in the PR description and epic body." Grep changes for `slice \w+`, `PR #\d+`, `issue #\d+`, bare `#\d+`, and URLs to GitHub issues inside `///` or `//` comments.

2. **Doc/code drift.** Module-level and function-level docs that reference values now made configurable, parameter names that moved, or behavioural claims invalidated by recent changes. Common shape: a function is refactored to take a new parameter, but the doc-comment still describes the old hardcoded value.

3. **Duplicated constants across modules.** If a string literal appears in N places (`"plonk.tools"`, `"trust-anchor.crt"`, namespace label keys, port numbers, etc.) there should be one `pub` or `pub(super)` constant and the rest should `use` it. Without this, a rename silently drifts.

4. **Hardcoded values that should be configurable.** Network addresses, DNS suffixes, ports, intervals, lifetimes — anything a real-world deployment might reasonably override should thread through `OperatorConfig` with an env var, default, and validation. The `.svc.cluster.local` regression on the mesh-config distributor is the canonical example of a silent breakage on clusters with custom `--cluster-domain`.

5. **Error type quality.** `Result<_, String>` blocks `?` propagation and forces `.map_err(|e| format!(...))` chains throughout the call site. The codebase convention is a small `thiserror` enum (mirrors `TrustAnchorError`, `TokenError`, `ReconcileError`).

6. **Observability gaps.** Spans wrapping infinite loops never close — instrument each iteration, not the loop. Important state transitions logged at INFO when they need WARN (production log pipelines filter INFO). Discarded return values that carry production signal (`distribute_to_namespaces` returns a count for a reason).

7. **Wire format / shared interface drift.** Field names that appear as literals in producer + consumer + test should be `pub(super) const`s referenced from all three. A rename in one place that doesn't update the others is silent breakage at runtime.

8. **Test gaps for boundary conditions.** Empty input, max input, error paths, and the implicit-success branch of conditionals. The `attempted == 0` no-op-cycle pin in `mesh_config_distributor` is canonical: a refactor to `applied == 0` alone would WARN on every empty cluster, but no test would have caught it without an explicit pin.

9. **Comments that explain WHAT instead of WHY.** CLAUDE.md is explicit: "Don't explain WHAT the code does, since well-named identifiers already do that." Comments should explain non-obvious decisions, hidden constraints, surprising behaviour, or workarounds for specific bugs.

10. **Reactive vs comprehensive.** When fixing any one comment, sweep every file in the change set, not just the file the comment is anchored to. The issue the reviewer flagged usually has siblings — and they often live in a different file the reviewer didn't pin a comment on. `feedback_class_level_fixes.md` captures this for both regression fixes ("grep siblings with the same wire shape") and review-comment fixes ("PR #215 round on `7d081c5` fixed the bot's 'not yet landed' phrase in `tests/e2e/identity.rs:13` but missed `services/trust_anchor.rs:58` ('not yet built') in the same diff because the sweep stayed within-file"). Within-file sweeps are the failure mode this checklist exists to break.

11. **Forward-looking / speculative claims in comments.** Phrases like "future X tooling will...", "slice 6b's injector needs...". These rot. Describe the *property* (single source of truth, contract, invariant) without naming speculative consumers.

12. **Rust API hygiene.** Idiomatic Rust hardening that lives outside CLAUDE.md but that the `claude[bot]` reviewer catches reliably. Self-review should walk these too:
    - **`#[must_use]` on return values that drive control flow.** Versions, cursors, `RequestOutcome`-like enums, nonces — anything whose discarding leaves the caller without a signal it was supposed to consume. Without the attribute, `let _ = f();` is silent; with it, a missed binding fails the build.
    - **Visibility scoping — lowest that compiles wins.** A struct with a private container (e.g. `WorldState::by_type` is private) doesn't need `pub` fields; `pub(super)` keeps the push loop's access while preventing a future accessor from leaking internals as crate-wide stable API.
    - **Intra-doc link syntax.** `[`name`]` resolves against free functions / modules in scope; method links must be explicit: `[`Type::method`]` or `[`name`](Self::name)`. Bare method names render as plain text and rustdoc warns "unresolved link."
    - **`#[tokio::test]` flavor.** The default is single-threaded — `tokio::join!` cooperates rather than parallelises. Tests claiming to exercise concurrency need `flavor = "multi_thread", worker_threads = N` AND `tokio::spawn` per future so they actually land on separate threads.
    - **`JoinHandle` error handling.** `let _ = tokio::join!(a, b)` silently drops `JoinError`s, including panics. Use `let (res_a, res_b) = tokio::join!(a, b); res_a.expect(...); res_b.expect(...)` so a panicking spawned task surfaces as a test failure.
    - **`HashMap::entry().or_default()` side effects.** Calling `.entry(k).or_default()` *before* an early return inserts a phantom entry. Functional accessors may still behave correctly, but `contains_key` is no longer a reliable proxy for "subscribed." Use `get_mut` on early-return paths; reserve `entry().or_default()` for create-on-write semantics.

13. **Symmetric-branch sweep.** When fixing one match arm or one direction of a paired API, audit the others for the same behaviour. The xDS NACK path not updating `subscribed_names` while the ACK path did was a real protocol correctness bug; `remove_cluster_does_not_cascade_to_endpoints` needed the EDS-side mirror. Always ask "if branch X does this, should branch Y also?"

14. **Re-read the diff after applying any fix.** AI-assisted patches commonly introduce drift adjacent to the fix — a doc-comment edit that adds a forward-looking phrase while removing one, an unused import left after a rewrite, a comment field name that drifted from the new code, a `let _ = ` site that the new `#[must_use]` exposes. `git diff` (not just the new file contents) before push, walking the *changed* lines with the checklist in mind, catches them.

15. **Doc-grounded verification for unfamiliar semantics.** When a fix involves an expression operator (type coercion, `==`/`!=`, logical operators on possibly-null inputs), a context variable (`github.event_name`, `inputs.X`, `needs.X` across invocation modes), an external resource shape (k8s `Deployment` vs `StatefulSet` condition types, Docker manifest formats, OCI image index vs schema-v2), or any other behaviour whose rules aren't visible in the code being changed: **stop and read the official docs before writing the fix**. Walk every input case (typically boolean true / boolean false / null / string / missing) through the documented rules and write down the expected result for each before committing to an expression. The cost is one doc-fetch round; the alternative is a multi-round review loop where each speculative fix introduces or preserves the same shape of bug because the underlying assumption was never validated. PR #377 ran four review rounds because the round-1 fix to a notify-suppression bug used `inputs.X != 'false'`, which under documented GitHub Actions coercion (`boolean → number`, `string → NaN`) evaluates as `0 != NaN` → `true` on the orchestrator-suppress case (i.e. the exact bug round 1 was supposed to fix). The doc-grounded re-audit on round 5 — fetching the GH expression-operator docs and walking every case — produced a `!inputs.suppress_X` form that worked unambiguously, closing the loop. The recipe is: when a fix touches semantics rather than logic, the docs come BEFORE the keyboard. See [[verify-dont-guess-semantics]].

## Rules

- **Never post to GitHub in draft mode.** Findings stay in the chat. The developer reads them, acts on them locally, and only triggers the actual bot review by flipping the PR to ready.
- **Reply on inline threads with the commit SHA** that actually contains the fix. The SHA-in-reply is what makes a fix verifiable end-to-end.
- **Resolve every thread you reply to** (Phase 2d step 7 / Phase 2e step 3). The next round's "unresolved bot threads" GraphQL query IS the working set — leaving threads open re-introduces the timestamp/pagination filter-drift class of bugs that have caused post-merge misses in prior rounds. SHA-reply without resolution is half a fix.
- **One comprehensive commit per file or logical unit** — not one commit per inline thread. The point of the holistic read is that fixes group naturally; the commit should reflect that grouping.
- **Run `cargo fmt`, `cargo clippy`, and the relevant `cargo test`** before pushing. CLAUDE.md is non-negotiable on these.
- **For changes touching the operator runtime, run `make e2e-full`** before declaring done. CLAUDE.md is also non-negotiable on this.
- **Never auto-merge or auto-mark-ready** even if every thread is addressed, even in `--auto` mode. The user does that.
- **Never skip a bot thread without explicit user direction.** A finding the assistant disagrees with should be declined with reasoning (Phase 2e), not silently dropped. Phase 2c is the dedicated triage step where the user picks fix vs decline per thread. **Invocation with `--auto` is itself the explicit user direction** to treat every bot finding as fix-bound — that mode skips Phase 2c by design, but the assistant still posts a reply (with the SHA) on every addressed thread, so nothing is silently dropped from the bot's perspective.
- **Autonomous loop: treat ambiguous as red; exit only on clearly green.** If the latest round's verdict is ambiguous ("LGTM but consider X", "minor nit", "optional"), apply the suggestion and run another round rather than declaring the loop done. The loop ends on a clearly green review or on user direction.
- **Re-read the `git diff` after every fix** before pushing, walking the changed lines through the checklist again. AI-assisted patches reliably introduce adjacent drift (an "if a future…" phrase added while removing another, a stale import, a comment field name that drifted) — one pass of `git diff` catches them and saves a re-review round.
- **Doc-grounded fixes when semantics are unfamiliar.** Whenever a fix involves rules that aren't visible in the code being changed (expression coercion, context variables across invocation modes, foreign type behaviour, language quirks in unfamiliar territory), **fetch the official docs first and walk every input case through the documented rules** before pushing — see Quality checklist item 15. The bot reviewer is a cross-check on your reasoning, not a substitute for understanding the spec. "Guessing it works" and "pushing and seeing" are the failure modes the autonomous loop is most prone to and the ones that produce the longest, most demoralising review chains. If you cannot point to the specific doc text or repo precedent that says your fix is correct, you don't yet know your fix is correct.
- **No AI references** in any commit message, PR comment, or reply. Same project rule as `/pr`.
