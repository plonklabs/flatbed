# /pr — Create or Update a Pull Request

## Description
Creates a PR with a structured description covering context, acceptance criteria, what was done, and test plan. Can also update an existing PR's description.

## Instructions

When the user runs `/pr` or `/pr <pr-number>`, follow this workflow:

### Phase 1: Gather Context

1. **Determine mode:**
   - `/pr` with no number: create a new PR
   - `/pr <number>`: update the description of an existing PR

2. **Read the branch state:**
   ```bash
   git log --oneline origin/main..HEAD
   git diff --stat origin/main..HEAD
   ```

3. **Read the full diff** to understand all changes:
   ```bash
   git diff origin/main..HEAD
   ```

4. **Check for linked issues** in commit messages (look for `#<number>`, `Part of #<number>`, `Closes #<number>`).

### Phase 2: Draft the Description

Write the PR description with these four sections:

#### Context
Explain **why** this change is being made. What problem does it solve? What prompted it? Write this as if the reader has no prior context — they should understand the motivation without reading the code.

#### Acceptance criteria
Bullet list of concrete, verifiable outcomes. What must be true for this PR to be considered complete? These should be testable statements, not vague goals.

#### What was done
Describe the implementation in plain language. Group related changes together. Explain design decisions and trade-offs where relevant. Don't just list files — explain what changed and why. Use paragraphs, not bullet lists, unless listing discrete independent changes.

#### Test plan
Checklist of verification steps. Mark completed items with `[x]`. Include:
- Unit/integration tests that were run
- Manual testing performed
- Edge cases verified

### Phase 3: Create or Update the PR

**For new PRs:**
1. Push the branch if not already pushed:
   ```bash
   git push -u origin <branch-name>
   ```
2. Create the PR:
   ```bash
   gh pr create --title "<title>" --body "<body>"
   ```

**For existing PRs:**
1. Update the description:
   ```bash
   gh pr edit <number> --title "<title>" --body "<body>"
   ```

### Rules

- **Title**: Under 70 characters. Lead with a verb (Add, Fix, Replace, Migrate). No period at the end.
- **Body**: Use a HEREDOC for the body to preserve formatting.
- **Tone**: Write as a human developer. No buzzwords, no filler. Be direct and specific.
- **No AI references**: Never mention AI tools, Claude, or add Co-Authored-By tags.
- Always show the user the drafted title and description before creating/updating.
- Return the PR URL when done.
