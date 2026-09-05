#!/usr/bin/env bash
# Contract fixtures for the flatbed `review-body` merge gate. Exercises every
# decision path the gate's contract names, across both verdict-sourcing paths:
#
#   review-object path (bot posted a review with commit_id == head):
#     clean pass, stale-SHA refusal, findings-unacked refusal, findings-acked
#     pass, partial-ack refusal, fail-closed uncertified-verdict refusal.
#
#   check-run + comment path (clean round: no review object, verdict is an
#   issue comment correlated to the `review / review` run at head — the comment
#   must fall inside the run's [started_at, completed_at] window):
#     clean-comment pass, findings-comment refuse/ack, comment-predates-run
#     refusal, straggler-after-completion refusal (a cancelled prior push's
#     late comment), no-successful-run refusal, no-comment refusal.
#
#   carried-over verdict (a force-push that changed no content: the shared
#   review workflow runs no model round and repeats the previous round's
#   `Verdict:` line under its own token, marked with the carried-over marker):
#     clean pass, findings refuse/ack, stale-window refusal, forged-by-a-human
#     refusal, unmarked-workflow-comment refusal.
#
# The fixtures carry `reviews`/`summaries`/`check_runs` inline so no `gh` call
# is made.
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
gate="$repo_root/scripts/merge-gates/review-body"

pass=0

run_gate() { printf '%s' "$1" | python3 "$gate"; }

expect_pass() { # label context [reason-substring]
    local label=$1 context=$2 want=${3:-}
    local out
    if ! out=$(run_gate "$context" 2>&1); then
        echo "FAILED $label: gate refused a valid fixture ($out)" >&2; exit 1
    fi
    if [[ -n "$want" && "$out" != *"$want"* ]]; then
        echo "FAILED $label: reason '$out' lacks '$want'" >&2; exit 1
    fi
    echo "ok   $label (pass: $out)"; pass=$((pass + 1))
}

expect_refuse() { # label context [reason-substring]
    local label=$1 context=$2 want=${3:-}
    local out
    if out=$(run_gate "$context" 2>&1); then
        echo "FAILED $label: gate passed an invalid fixture ($out)" >&2; exit 1
    fi
    if [[ -n "$want" && "$out" != *"$want"* ]]; then
        echo "FAILED $label: reason '$out' lacks '$want'" >&2; exit 1
    fi
    echo "ok   refused $label ($out)"; pass=$((pass + 1))
}

head='deadbeef'
review_at_head='{"user":{"login":"claude[bot]"},"commit_id":"deadbeef","state":"COMMENTED"}'
review_at_old='{"user":{"login":"claude[bot]"},"commit_id":"0ldc0de","state":"COMMENTED"}'
clean_summary='{"user":{"login":"claude[bot]"},"body":"## Review\n\nReady to merge. No blocking concerns.","created_at":"2026-09-01T10:00:00Z"}'
two_findings='{"user":{"login":"claude[bot]"},"body":"## Review\n\n**Findings**\n\n**Finding 1 (correctness):** wrong SHA compared.\n**Finding 2 (hygiene):** stray comment.","created_at":"2026-09-01T10:00:00Z"}'
uncertified='{"user":{"login":"claude[bot]"},"body":"## Review\n\nSome remarks about the diff with no verdict line.","created_at":"2026-09-01T10:00:00Z"}'

# Pinned `Verdict:` marker fixtures. The marker line is authoritative and read
# before any prose heuristic. `verdict_clean`'s prose ("everything looks fine
# here") matches no CLEAN_MARKERS entry, so the `Verdict: clean` line is the
# only thing that can certify it — isolating the marker path from the prose
# fallback; `verdict_findings` declares findings and enumerates them;
# `verdict_findings_bare` declares findings with no `Finding N` label, so the
# fail-closed guard must yield the generic finding; `legacy_variant` carries
# only a recurring clean phrase with no marker line (fallback path);
# `verdict_prose` mentions "verdict" mid-sentence with no marker line and no
# clean phrase, so it must fail closed rather than match.
verdict_clean='{"user":{"login":"claude[bot]"},"body":"## Review\n\nEverything looks fine here.\n\nVerdict: clean","created_at":"2026-09-01T10:00:00Z"}'
verdict_findings='{"user":{"login":"claude[bot]"},"body":"## Review\n\n**Finding 1 (correctness):** unchecked unwrap.\n**Finding 2 (hygiene):** stray dbg!.\n\nVerdict: findings","created_at":"2026-09-01T10:00:00Z"}'
verdict_findings_bare='{"user":{"login":"claude[bot]"},"body":"## Review\n\nThere are problems here I could not enumerate cleanly.\n\nVerdict: findings","created_at":"2026-09-01T10:00:00Z"}'
legacy_variant='{"user":{"login":"claude[bot]"},"body":"## Review\n\nRe-checked the delta — no other issues introduced.","created_at":"2026-09-01T10:00:00Z"}'
verdict_prose='{"user":{"login":"claude[bot]"},"body":"## Review\n\nThe verdict is still out on whether this scales, but I have no marker to give.","created_at":"2026-09-01T10:00:00Z"}'

# check-run + comment path fixtures. The `review / review` run at head spans
# [10:00, 10:10]; the verdict comment (posted before the run concludes) lands
# inside the window at 10:05. A comment before 10:00 predates the run; a
# comment after 10:10 is a straggler from a cancelled prior-push run.
review_run_ok='{"name":"review / review","status":"completed","conclusion":"success","started_at":"2026-09-01T10:00:00Z","completed_at":"2026-09-01T10:10:00Z"}'
review_run_skipped='{"name":"review / review","status":"completed","conclusion":"skipped","started_at":"2026-09-01T10:00:00Z","completed_at":"2026-09-01T10:00:00Z"}'
clean_comment_fresh='{"user":{"login":"claude[bot]"},"body":"## Review\n\nLGTM — ready to merge.","created_at":"2026-09-01T10:05:00Z"}'
findings_comment_fresh='{"user":{"login":"claude[bot]"},"body":"## Review\n\n**Finding 1:** off-by-one.\n**Finding 2:** stray import.","created_at":"2026-09-01T10:05:00Z"}'
clean_comment_before='{"user":{"login":"claude[bot]"},"body":"## Review\n\nLGTM — ready to merge.","created_at":"2026-09-01T09:00:00Z"}'
clean_comment_straggler='{"user":{"login":"claude[bot]"},"body":"## Review\n\nLGTM — ready to merge.","created_at":"2026-09-01T10:20:00Z"}'

# Carried-over verdict fixtures. The review workflow posts these under the
# Actions token when a force-push rewrote SHAs without changing content, so the
# author is `github-actions[bot]` rather than the reviewer; the carried-over
# marker plus that login plus the run window are what qualify the comment.
# `carried_forged` is the same body from a human account — a PR author can put
# the marker in a comment box, but not the login behind it. `workflow_unmarked`
# is the round-ceiling notice the same workflow posts: same author, no marker,
# so it certifies nothing.
carried_marker='<!-- claude-review-carried-over -->'
carried_clean='{"user":{"login":"github-actions[bot]"},"body":"'"$carried_marker"'\n<!-- claude-review-patch-ids: a1b2 -->\n**No content change since review round 2.**\n\nVerdict: clean","created_at":"2026-09-01T10:05:00Z"}'
carried_findings='{"user":{"login":"github-actions[bot]"},"body":"'"$carried_marker"'\n<!-- claude-review-patch-ids: a1b2 -->\n**No content change since review round 2.**\n\nVerdict: findings","created_at":"2026-09-01T10:05:00Z"}'
carried_stale='{"user":{"login":"github-actions[bot]"},"body":"'"$carried_marker"'\n\nVerdict: clean","created_at":"2026-09-01T09:00:00Z"}'
carried_forged='{"user":{"login":"someauthor"},"body":"'"$carried_marker"'\n\nVerdict: clean","created_at":"2026-09-01T10:05:00Z"}'
workflow_unmarked='{"user":{"login":"github-actions[bot]"},"body":"<!-- claude-review-round-ceiling -->\n**Review round ceiling reached; all bot threads resolved.**","created_at":"2026-09-01T10:05:00Z"}'

ctx() { # head acks reviews summaries [check_runs]
    printf '{"pr":61,"repo":"plonklabs/flatbed","base":"main","head_sha":"%s","acks":%s,"reviews":[%s],"summaries":[%s],"check_runs":[%s]}' \
        "$1" "$2" "$3" "$4" "${5:-}"
}

# --- review-object path: a review must exist at the exact head SHA ----------
expect_pass 'clean review at head'          "$(ctx "$head" '[]' "$review_at_head" "$clean_summary")" 'clean at head'
expect_refuse 'clean verdict on a stale SHA' "$(ctx "$head" '[]' "$review_at_old" "$clean_summary")" 'no review object'

# --- review-object path: findings must be acknowledged ----------------------
expect_refuse 'two findings, no acks'       "$(ctx "$head" '[]' "$review_at_head" "$two_findings")"          '2 unacknowledged'
expect_pass 'two findings, two acks'        "$(ctx "$head" '["flaky","by design"]' "$review_at_head" "$two_findings")" 'acknowledged'
expect_refuse 'two findings, one ack'       "$(ctx "$head" '["flaky"]' "$review_at_head" "$two_findings")"   '1 unacknowledged'

# --- review-object path: fail closed on an unrecognized verdict -------------
expect_refuse 'uncertified verdict, no ack' "$(ctx "$head" '[]' "$review_at_head" "$uncertified")"          'not certified clean'
expect_pass 'uncertified verdict acked'     "$(ctx "$head" '["reviewed manually"]' "$review_at_head" "$uncertified")" 'acknowledged'

# --- pinned `Verdict:` marker takes precedence over prose -------------------
expect_pass 'explicit Verdict: clean'       "$(ctx "$head" '[]' "$review_at_head" "$verdict_clean")"         'clean at head'
expect_refuse 'explicit Verdict: findings'  "$(ctx "$head" '[]' "$review_at_head" "$verdict_findings")"      '2 unacknowledged'
expect_pass 'Verdict: findings, acked'      "$(ctx "$head" '["by design","by design"]' "$review_at_head" "$verdict_findings")" 'acknowledged'
expect_refuse 'Verdict: findings, none enumerated' "$(ctx "$head" '[]' "$review_at_head" "$verdict_findings_bare")" 'review findings reported'
expect_pass 'legacy clean marker, no marker line' "$(ctx "$head" '[]' "$review_at_head" "$legacy_variant")" 'clean at head'
expect_refuse 'prose says verdict, no marker' "$(ctx "$head" '[]' "$review_at_head" "$verdict_prose")"       'not certified clean'

# --- check-run + comment path: clean round posts an issue comment -----------
expect_pass 'clean issue-comment round'     "$(ctx "$head" '[]' '' "$clean_comment_fresh" "$review_run_ok")" 'clean at head'
expect_refuse 'findings comment, no acks'   "$(ctx "$head" '[]' '' "$findings_comment_fresh" "$review_run_ok")" '2 unacknowledged'
expect_pass 'findings comment, acked'       "$(ctx "$head" '["by design","by design"]' '' "$findings_comment_fresh" "$review_run_ok")" 'acknowledged'

# --- check-run + comment path: the comment must fall inside the run window --
expect_refuse 'comment predates the run'    "$(ctx "$head" '[]' '' "$clean_comment_before" "$review_run_ok")" 'stale'
expect_refuse 'straggler after completion'  "$(ctx "$head" '[]' '' "$clean_comment_straggler" "$review_run_ok")" 'stale'

# --- check-run + comment path: no verdict at all ----------------------------
expect_refuse 'no run, no review object'    "$(ctx "$head" '[]' '' "$clean_comment_fresh")"                'no successful'
expect_refuse 'skipped run does not count'  "$(ctx "$head" '[]' '' "$clean_comment_fresh" "$review_run_skipped")" 'no successful'
expect_refuse 'run but no verdict comment'  "$(ctx "$head" '[]' '' '' "$review_run_ok")"                   'no comment falls within'

# --- the run window bounds the review-object path too -----------------------
expect_refuse 'review at head, straggler comment' "$(ctx "$head" '[]' "$review_at_head" "$clean_comment_straggler" "$review_run_ok")" 'no comment falls within'

# --- carried-over verdict: a rebase that changed no content still certifies -
expect_pass 'carried-over clean verdict'    "$(ctx "$head" '[]' '' "$carried_clean" "$review_run_ok")"      'carried over across a content-free push'
expect_refuse 'carried-over findings, no ack' "$(ctx "$head" '[]' '' "$carried_findings" "$review_run_ok")" '1 unacknowledged'
expect_pass 'carried-over findings, acked'  "$(ctx "$head" '["by design"]' '' "$carried_findings" "$review_run_ok")" 'carried over across a content-free push'

# --- carried-over verdict: window and authorship still gate it --------------
expect_refuse 'carried-over outside the window' "$(ctx "$head" '[]' '' "$carried_stale" "$review_run_ok")"  'no comment falls within'
expect_refuse 'human forging the carried marker' "$(ctx "$head" '[]' '' "$carried_forged" "$review_run_ok")" 'no comment falls within'
expect_refuse 'workflow comment without the marker' "$(ctx "$head" '[]' '' "$workflow_unmarked" "$review_run_ok")" 'no comment falls within'

echo "validated $pass review-body gate contract paths"
