# /review — Self-review a draft PR or address bot review comments

## Description

Two-mode workflow keyed off PR state:

- **Draft PR** → the assistant performs a local self-review against the [Quality checklist](#quality-checklist) and prints findings in the chat. Nothing is posted to GitHub. The point is to surface issues *before* flipping to ready, so the developer can fix them before the `claude[bot]` reviewer (configured in `.github/workflows/claude-review.yml`) sees them.
- **Ready-for-review PR** → the assistant waits for the bot's review run for the current HEAD commit to finish, then fetches its inline + top-level comments and runs a holistic fix loop. The fix loop reads each affected file in full (not just diff hunks) and applies the [Quality checklist](#quality-checklist) to catch siblings of every flagged issue.

## Instructions

When the user runs `/review` or `/review <pr-number>`, follow this workflow:

### Phase 0: Detect PR state

```bash
PR_NUMBER=${1:-$(gh pr view --json number --jq '.number')}
gh pr view "$PR_NUMBER" --json isDraft,state,headRefOid,headRefName
```

- If `isDraft == true` → **Phase 1** (draft mode).
- If `isDraft == false && state == "OPEN"` → **Phase 2** (ready mode).
- If `state` is `MERGED` or `CLOSED`, ask the user whether they want to review historical comments anyway, then go to Phase 2 (skipping the wait sub-phase).

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

#### Phase 2b: Fetch comments

GitHub Apps appear in the REST API with a `<name>[bot]` login, so the filter is `claude[bot]` — not the bare app name.

```bash
# Inline (line-attached) comments from the bot.
gh api "repos/winkoz/plonk/pulls/$PR_NUMBER/comments" \
    --jq '.[] | select(.user.login == "claude[bot]") |
          {id, path, line, body, commit_id, created_at}'

# Top-level review summaries from the bot.
gh api "repos/winkoz/plonk/issues/$PR_NUMBER/comments" \
    --jq '.[] | select(.user.login == "claude[bot]") |
          {id, body, created_at}'
```

**Multi-round PRs**: every push to a non-draft PR re-triggers the bot workflow, so a re-review may include comments that were already addressed in a previous round. Two ways to identify the new ones:

- **By thread state**: an inline comment is "addressed" if there's a reply on its thread (`gh api repos/winkoz/plonk/pulls/$PR_NUMBER/comments/<id>/replies`) whose body references a commit SHA that's already in the PR's history. New comments don't have such a reply.
- **By timestamp**: get the timestamp of the most recent fix-reply commit (`git log --format=%cI -1 -- <path>` or the timestamp of the last push), and filter `created_at > $timestamp`.

If you can't tell, process all bot comments — it's cheaper to no-op on an already-addressed one than to silently skip a new one.

If there are zero bot comments, report "no bot comments to address" and stop.

#### Phase 2c: Triage with the user

Before applying any fix, present the bot's comments to the user grouped by file, with a one-line summary of each. For each comment (or batch of related comments on the same file), ask the user whether to **fix** or **decline**. Don't proceed to Phase 2d until the user has chosen for every comment.

This step exists because the Rules below require explicit user direction before fixing or skipping a comment. Skipping this step and going straight to fixes denies the user that decision point.

#### Phase 2d: Apply fixes holistically

For each batch of fix-bound comments on the **same file**:

1. **Read the FULL file** (not just the lines around the comment). The reactive line-fix loop is the failure mode that drives multi-round review iteration: every round introduces a new pinpoint patch that leaves adjacent issues uncovered.
2. **Read the immediate neighbours** the comment alludes to. If the comment is about a constant duplication, also open the file holding the canonical constant. If it's about a doc/code drift, also open the docs that reference the changed value.
3. **Walk the [Quality checklist](#quality-checklist)** and fix the comment AND any adjacent issues that fall under the same category. The sweep must span **every file in the change set** (`git diff origin/main..HEAD --name-only`), not just the file the comment is anchored to. A "constant duplication" comment on one literal should trigger a sweep for that pattern in every changed file; a "hardcoded value" comment, a "forward-looking phrase" comment, an "error type quality" comment — all the same. Within-file sweeps miss cross-file siblings, which is the failure mode `feedback_class_level_fixes.md` captures and which has shipped regressions on this codebase.
4. **Stage and commit** the comprehensive fix as **one commit per file or per logical unit** — not one commit per inline comment. The commit message should describe the comprehensive change, not just the comment that triggered it.
5. **Run the relevant verifications before pushing:**
   - `cargo fmt --all` (apply formatting; AI-written code often needs normalisation, and `cargo fmt --all -- --check` would just fail rather than fix)
   - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
   - `cargo test -p <affected-package> --bins` (or workspace tests if the change is wide)
   - For changes touching the operator runtime: `make e2e-full` per CLAUDE.md

   **If any check fails, stop. Surface the output to the user, do not push, and do not paper over the failure with hand-edits that don't address the underlying cause.** A failed `cargo test` means the fix introduced a regression; a failed `clippy` means the fix introduced new style debt; both are blockers for the SHA-in-reply flow because the SHA being referenced wouldn't pass CI on `main`. Iterate on the fix until verifications pass, then push.
6. **Push.** Then reply on each addressed inline thread with the commit SHA:

   ```bash
   gh api -X POST \
       "repos/winkoz/plonk/pulls/$PR_NUMBER/comments/<comment-id>/replies" \
       -f body='Fixed in <commit-sha>. <one-sentence what changed and why it covers more than the line>'
   ```

#### Phase 2e: Decline path

For each comment the user marked as **decline** in Phase 2c:

1. Ask for the reasoning (or draft a response and confirm with the user before posting).
2. Reply on the thread with the rationale (no commit SHA needed):

   ```bash
   gh api -X POST \
       "repos/winkoz/plonk/pulls/$PR_NUMBER/comments/<comment-id>/replies" \
       -f body='<reasoning>'
   ```

#### Phase 2f: Post the top-level summary

Run *after* both Phase 2d and Phase 2e have completed — the summary references state from both (fixed comments, declined comments) and posting it earlier means claiming a state that hasn't yet been produced.

```bash
gh pr comment "$PR_NUMBER" --body "<summary>"
```

The summary should cover:
- Reviewer-flagged issues fixed (with commit SHAs)
- Self-flagged issues fixed in the same pass (the holistic delta)
- Comments declined (with reasoning)

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

## Rules

- **Never post to GitHub in draft mode.** Findings stay in the chat. The developer reads them, acts on them locally, and only triggers the actual bot review by flipping the PR to ready.
- **Reply on inline threads with the commit SHA** that actually contains the fix. The SHA-in-reply is what makes a fix verifiable end-to-end.
- **One comprehensive commit per file or logical unit** — not one commit per inline comment. The point of the holistic read is that fixes group naturally; the commit should reflect that grouping.
- **Run `cargo fmt`, `cargo clippy`, and the relevant `cargo test`** before pushing. CLAUDE.md is non-negotiable on these.
- **For changes touching the operator runtime, run `make e2e-full`** before declaring done. CLAUDE.md is also non-negotiable on this.
- **Never auto-merge or auto-mark-ready** even if every comment is addressed. The user does that.
- **Never skip a comment without explicit user direction.** A comment the assistant disagrees with should be declined with reasoning (Phase 2e), not silently dropped. Phase 2c is the dedicated triage step where the user picks fix vs decline per comment.
- **Re-read the `git diff` after every fix** before pushing, walking the changed lines through the checklist again. AI-assisted patches reliably introduce adjacent drift (an "if a future…" phrase added while removing another, a stale import, a comment field name that drifted) — one pass of `git diff` catches them and saves a re-review round.
- **No AI references** in any commit message, PR comment, or reply. Same project rule as `/pr`.
