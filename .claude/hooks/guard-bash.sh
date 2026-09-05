#!/usr/bin/env bash
# PreToolUse (Bash): refuses the commands the repository policy forbids
# outright, so the rule is enforced by the harness instead of restated in
# every skill. Reads the tool input on stdin, prints a deny decision with the
# reason on stdout, and stays silent (exit 0, no output) for everything else.
set -euo pipefail

root="${CLAUDE_PROJECT_DIR:-.}"
cmd="$(jq -r '.tool_input.command // empty' 2>/dev/null || true)"
[ -z "$cmd" ] && exit 0

deny() {
  jq -n --arg reason "$1" '{hookSpecificOutput: {hookEventName: "PreToolUse", permissionDecision: "deny", permissionDecisionReason: $reason}}'
  exit 0
}

# Pull requests open as drafts; the draft state is the self-review window.
if grep -Eq '(^|[;&|[:space:]])gh[[:space:]]+pr[[:space:]]+create([[:space:]]|$)' <<<"$cmd" \
   && ! grep -Eq -- '(^|[[:space:]])(--draft|-d)([[:space:]]|$)' <<<"$cmd"; then
  deny "gh pr create must carry --draft; the draft state is the self-review window."
fi

# A merge is authorized by a passing `fleet merge <n> --no-merge` and lands
# through the pinned two-step form. Anything that skips the head pin or the
# admin bypass is not that path.
if grep -Eq '(^|[;&|[:space:]])gh[[:space:]]+pr[[:space:]]+merge([[:space:]]|$)' <<<"$cmd" \
   && ! { grep -Eq -- '(^|[[:space:]])--admin([[:space:]]|$)' <<<"$cmd" \
          && grep -Eq -- '(^|[[:space:]])--match-head-commit([[:space:]=]|$)' <<<"$cmd"; }; then
  deny "Merges are authorized by 'fleet merge <n> --no-merge' and land as 'gh pr merge <n> --squash --admin --match-head-commit <sha>'; a merge without the head pin is not that path."
fi

# These gh subcommands wrap GraphQL and are matched only in command
# position — quoted strings, heredoc bodies, and comments are stripped
# first so prose that merely mentions a subcommand is not denied.
strip_prose() {
  awk '
    BEGIN {
      in_heredoc = 0
      delim = ""
      q = sprintf("%c", 39)
      dq = "\""
      heredoc_re = "<<-?[ \t]*[" dq q "]?[A-Za-z_][A-Za-z0-9_]*[" dq q "]?"
      quote_re = "[" dq q "]"
    }
    {
      line = $0
      if (in_heredoc) {
        trimmed = line
        gsub(/^[ \t]+/, "", trimmed)
        if (trimmed == delim) { in_heredoc = 0 }
        next
      }
      if (match(line, heredoc_re)) {
        tok = substr(line, RSTART, RLENGTH)
        gsub(/<<-?[ \t]*/, "", tok)
        gsub(quote_re, "", tok)
        delim = tok
        in_heredoc = 1
        print substr(line, 1, RSTART - 1)
        next
      }
      print line
    }
  ' <<<"$1" | sed -E 's/"[^"]*"//g; s/'"'"'[^'"'"']*'"'"'//g; s/#.*$//'
}
stripped="$(strip_prose "$cmd")"

if grep -Eq '(^|[;&|[:space:]])gh[[:space:]]+pr[[:space:]]+view([[:space:]]|$)' <<<"$stripped"; then
  deny "gh pr view wraps GraphQL; use gh api repos/{owner}/{repo}/pulls/{n} instead."
fi
if grep -Eq '(^|[;&|[:space:]])gh[[:space:]]+pr[[:space:]]+checks([[:space:]]|$)' <<<"$stripped"; then
  deny "gh pr checks wraps GraphQL; use gh api repos/{owner}/{repo}/commits/{sha}/check-runs instead."
fi
if grep -Eq '(^|[;&|[:space:]])gh[[:space:]]+pr[[:space:]]+list([[:space:]]|$)' <<<"$stripped"; then
  deny 'gh pr list wraps GraphQL; use gh api "repos/{owner}/{repo}/pulls?state=open" instead.'
fi
if grep -Eq '(^|[;&|[:space:]])gh[[:space:]]+issue[[:space:]]+(list|view)([[:space:]]|$)' <<<"$stripped"; then
  deny "gh issue list/view wraps GraphQL; use gh api repos/{owner}/{repo}/issues -X GET -f state=open -f labels=... (label filters need -f: emoji and spaces must be encoded) or repos/{owner}/{repo}/issues/{n} instead."
fi
if grep -Eq '(^|[;&|[:space:]])gh[[:space:]]+issue[[:space:]]+create([[:space:]]|$)' <<<"$stripped"; then
  deny "gh issue create wraps GraphQL; use gh api repos/{owner}/{repo}/issues -f title=... -F body=@file instead."
fi
if grep -Eq '(^|[;&|[:space:]])gh[[:space:]]+repo[[:space:]]+view([[:space:]]|$)' <<<"$stripped"; then
  deny "gh repo view wraps GraphQL; use gh api repos/{owner}/{repo} instead."
fi
if grep -Eq '(^|[;&|[:space:]])gh[[:space:]]+run[[:space:]]+view([[:space:]].*)?[[:space:]]--json([[:space:]=]|$)' <<<"$stripped"; then
  deny "gh run view --json wraps GraphQL; use gh api repos/{owner}/{repo}/actions/runs/{id} instead."
fi
if grep -Eq '(^|[;&|[:space:]])gh[[:space:]]+[^;&|[:space:]]*[[:space:]].*--watch([[:space:]]|$)' <<<"$stripped"; then
  deny "--watch holds a session open polling GraphQL; use a bounded gh api repos/{owner}/{repo}/commits/{sha}/check-runs (or actions/runs/{id}) poll instead."
fi

# Commits never land on main; every change arrives through a feature branch.
if grep -Eq '(^|[;&|[:space:]])git[[:space:]]+(-C[[:space:]]+[^[:space:]]+[[:space:]]+)?commit([[:space:]]|$)' <<<"$cmd"; then
  dir="$(grep -Eo 'git[[:space:]]+-C[[:space:]]+[^[:space:]]+' <<<"$cmd" | head -1 | awk '{print $3}' || true)"
  branch="$(git -C "${dir:-$root}" branch --show-current 2>/dev/null || true)"
  if [ "$branch" = "main" ]; then
    deny "HEAD is on 'main' ($(git -C "${dir:-$root}" log -1 --oneline 2>/dev/null)); cut a feature branch before committing."
  fi
fi

# A branch behind main is rebased, never merged: no merge commits in a PR.
if grep -Eq '(^|[;&|[:space:]])git[[:space:]]+(-C[[:space:]]+[^[:space:]]+[[:space:]]+)?merge([[:space:]]|$)' <<<"$cmd" \
   && ! grep -Eq -- '--(abort|continue|quit)([[:space:]]|$)' <<<"$cmd"; then
  deny "Branches are rebased onto main, never merged: use git rebase origin/main && git push --force-with-lease."
fi
if grep -Eq '(^|[;&|[:space:]])git[[:space:]]+(-C[[:space:]]+[^[:space:]]+[[:space:]]+)?pull([[:space:]]|$)' <<<"$cmd" \
   && ! grep -Eq -- '(--(rebase|ff-only)([[:space:]=]|$)|(^|[[:space:]])-r([[:space:]]|$))' <<<"$cmd"; then
  deny "git pull without --rebase or --ff-only can create a merge commit; rebase instead."
fi
if grep -Eq 'pulls/[^/[:space:]]+/update-branch' <<<"$cmd"; then
  deny "GitHub update-branch merges main into the PR; rebase the branch and push --force-with-lease instead."
fi

exit 0
