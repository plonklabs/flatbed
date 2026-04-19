# /topr — Rebase a Stacked PR onto origin/main

## Description
Checks out a PR branch, rebases it onto the latest `origin/main` (dropping already-merged commits from earlier PRs in the stack), and force-pushes.

**This repo uses squash-and-merge.** A naive `git rebase origin/main` will NOT drop already-merged commits because squash-merge creates different SHAs on main. Instead, we must identify the commits unique to this PR and cherry-pick only those onto origin/main.

## Arguments
- `$ARGUMENTS` — the PR number (e.g. `103`)

## Instructions

When the user runs `/topr <PR>`, execute the following steps:

1. **Get PR metadata:**
   ```bash
   gh pr view <PR> --json headRefName,title,state,commits --jq '.'
   ```
   If the PR is not open, warn the user and stop.
   Note the number of commits reported by the PR — this is the count of commits unique to this PR.

2. **Fetch latest main and the PR branch:**
   ```bash
   git fetch origin main <branch>
   ```

3. **Check out the branch:**
   ```bash
   git checkout <branch>
   ```

4. **Identify commits unique to this PR:**

   This is the critical step. Since we squash-merge, `origin/main` contains squashed versions of earlier PRs in the stack, but git doesn't know they're the same patches. We need to figure out which commits on the branch are actually new in this PR vs carried over from earlier (already-merged) PRs.

   **Strategy: Use `git diff` to find the unique commits.**

   a. First, show all commits above main:
      ```bash
      git log --oneline origin/main..HEAD
      ```

   b. Compare the tree diff against main to understand what this PR actually changes:
      ```bash
      git diff --stat origin/main..HEAD
      ```

   c. Look at the PR's commit count from step 1. The PR's own commits are the **most recent N** commits (where N = PR commit count). The older commits below them are from earlier PRs in the stack.

   d. If the PR commit count is not available or ambiguous, identify unique commits by checking which files each commit touches vs what's already on main. Commits whose changes are already present on main (via squash-merge) are stale.

   e. List the unique commits (most recent N):
      ```bash
      git log --oneline -N HEAD  # where N is the PR's commit count
      ```

5. **Reset to origin/main and cherry-pick unique commits:**
   ```bash
   git reset --hard origin/main
   git cherry-pick <commit1> <commit2> ...  # in chronological order (oldest first)
   ```
   - If cherry-pick has conflicts, stop and show them. Ask the user how to proceed.
   - **Do NOT use `git rebase origin/main`** — it replays all commits and won't drop squash-merged duplicates.

6. **Verify the result:**
   ```bash
   git log --oneline origin/main..HEAD
   git diff --stat origin/main..HEAD
   ```
   Confirm that:
   - Only the PR's own commits remain (no stale commits from earlier PRs)
   - The diff matches what the PR is supposed to change

7. **Force-push:**
   ```bash
   git push --force-with-lease origin <branch>
   ```

8. **Report the result:**
   - Confirm the PR is rebased and pushed.
   - Show the final `git log --oneline origin/main..HEAD`.
   - Show how many commits were kept vs dropped.
