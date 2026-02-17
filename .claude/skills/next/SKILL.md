# /next — Reset Session to Start Fresh

## Description
Resets the working tree to the `dummy` branch, rebased on latest `main`. Use this between tasks to start with a clean slate.

## Instructions

When the user runs `/next`, execute the following steps:

1. **Fetch latest main and switch to dummy:**
   ```bash
   git fetch origin main
   ```
   Then switch to the `dummy` branch. If it doesn't exist yet, create it:
   ```bash
   git switch dummy || git switch -c dummy origin/main
   ```
   Then reset to latest main:
   ```bash
   git reset --hard origin/main
   ```
   This works in worktrees where `main` is checked out by the parent tree. The `dummy` branch exists solely as a parking branch for worktree sessions.

2. **Report the result:**
   - On success: Confirm the branch is ready and show the latest commit on `main`
   - On conflict or error: Show what went wrong and ask the user how to proceed

3. **Prompt for next task:** Ask the user what they'd like to work on next.
