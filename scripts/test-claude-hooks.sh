#!/usr/bin/env bash
# Contract fixtures for .claude/hooks/guard-bash.sh: each forbidden command
# shape is denied with a reason, and the allowed shapes pass silently.
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
hook="$repo_root/.claude/hooks/guard-bash.sh"
fail() { echo "FAIL: $1" >&2; exit 1; }

run() {
  jq -n --arg c "$1" '{tool_input: {command: $c}}' | CLAUDE_PROJECT_DIR="$repo_root" bash "$hook"
}
decision() { out="$(run "$1")"; [ -z "$out" ] && { echo allow; return; }; jq -r '.hookSpecificOutput.permissionDecision // "allow"' <<<"$out"; }

expect() { # <deny|allow> <command>
  got="$(decision "$2")"
  [ "$got" = "$1" ] || fail "expected $1 for: $2 (got $got)"
}

# PRs open as drafts.
expect deny  'gh pr create --title "Add a thing" --body-file b.md'
expect allow 'gh pr create --draft --title "Add a thing" --body-file b.md'

# The merge is the pinned two-step form or nothing.
expect deny  'gh pr merge 12 --squash'
expect deny  'gh pr merge 12 --squash --admin'
expect allow 'gh pr merge 12 --squash --admin --match-head-commit abc123 --subject "t" --body "b"'
expect allow 'fleet merge 12 --no-merge'

# GitHub reads/writes go over REST; these gh subcommands wrap GraphQL.
expect deny  'gh pr view 12'
expect allow 'gh api repos/plonklabs/flatbed/pulls/12'
expect deny  'gh pr checks 12'
expect deny  'gh pr list --state open'
expect deny  'gh issue list --state open'
expect deny  'gh issue view 61'
expect deny  'gh issue create --title x --body y'
expect deny  'gh repo view'
expect deny  'gh run view 123 --json status,conclusion'
expect allow 'gh run view 123'
expect deny  'gh pr checks 12 --watch'
expect allow 'gh pr ready 12'

# A multi-line command whose forbidden token and flag land on different
# lines is not one forbidden call — each grep must stay scoped to a line.
json_split='gh run view 123
cargo test --workspace --json'
expect allow "$json_split"
watch_split='gh api repos/plonklabs/flatbed/pulls/12
echo start --watch here'
expect allow "$watch_split"
expect allow 'gh run list --workflow=claude-review.yml --limit 12 --json databaseId,headSha,conclusion'

# Quoted prose and heredoc bodies merely mentioning a forbidden subcommand
# are not command position. Every guard is checked, not just the GraphQL
# group: a guard reading the raw command falsely denies a script that writes
# the forbidden form as text.
expect allow 'echo "reminder: never run gh pr view or gh issue list here"'
expect allow 'fleet brief --issue 12 --extra "avoid gh pr view and gh issue list"'
heredoc_cmd='cat <<EOF
Remember not to use gh pr view or gh issue list.
EOF'
expect allow "$heredoc_cmd"
expect allow "gh api repos/plonklabs/flatbed/pulls/12 --jq '.title' # not gh issue list"
expect allow 'echo "always: gh pr create --draft, never a bare gh pr create"'
expect allow 'fleet brief --issue 12 --extra "do not run gh pr merge 12 --squash"'
expect allow 'echo "rebase; never git merge origin/main or git pull origin main"'
release_doc='cat <<EOF
Release steps:
  gh pr create --title "release" --body "notes"
  gh pr merge 12 --squash
  git merge origin/main
EOF'
expect allow "$release_doc"

# Stripping must not let a quoted flag excuse a real command: the flag and the
# subcommand it qualifies are read from the same stripped text.
expect deny  'gh pr create --title "uses --draft in the title"'
expect deny  'gh pr merge 12 --squash --subject "--admin --match-head-commit"'
expect deny  'git pull origin main # --rebase would have been right'

run 'gh pr view 12' | jq -e '.hookSpecificOutput.permissionDecisionReason | test("pulls/\\{n\\}")' >/dev/null || fail "pr view denial carries no REST form"
run 'gh pr merge 1' | jq -e '.hookSpecificOutput.permissionDecisionReason | test("fleet merge")' >/dev/null || fail "merge denial carries no reason"

# Branches rebase onto main; they never merge it.
expect deny  'git merge origin/main'
expect allow 'git merge --abort'
expect deny  'git pull origin main'
expect allow 'git pull --rebase origin main'
expect allow 'git pull --ff-only origin main'
expect allow 'git pull -r origin main'
expect deny  'gh api -X PUT repos/plonklabs/flatbed/pulls/12/update-branch'
expect allow 'git fetch origin main && git rebase origin/main && git push --force-with-lease'

# Ordinary work is untouched.
expect allow 'cargo test --workspace'
expect allow 'bash scripts/check-generated.sh'

# Commit guard: a throwaway repo on main denies, on a feature branch allows.
tmp="$(mktemp -d)"
git -C "$tmp" init -q -b main && git -C "$tmp" -c user.email=t@t -c user.name=t commit -q --allow-empty -m init
expect deny  "git -C $tmp commit -m x"
git -C "$tmp" switch -q -c feature/x
expect allow "git -C $tmp commit -m x"
rm -rf "$tmp"

echo "validated guard-bash hook fixtures"
