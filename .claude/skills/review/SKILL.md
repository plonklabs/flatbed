# /review — Address PR Review Comments

## Description
Interactive workflow for fetching, triaging, and responding to PR review comments. Each comment is fixed or declined with an inline reply.

## Instructions

When the user runs `/review <pr-number>`, follow this workflow:

### Phase 1: Fetch and Present Comments

1. **Fetch review comments:**
   ```bash
   gh api repos/winkoz/plonk/pulls/<pr-number>/comments
   ```

2. **Group comments by file** and present them in a numbered list. For each comment show:
   - File path and line number
   - Reviewer name
   - Comment body (summarized if long)

3. If there are no comments, report that and stop.

### Phase 2: Triage Each Comment

For each comment, ask the user whether to:
- **Fix**: Implement the change
- **Decline**: Provide reasoning

Process comments one at a time (or in small batches if they relate to the same file/concern).

### Phase 3: Apply Fixes

For each comment marked as **fix**:

1. Make the code change
2. Stage and commit with a clear message
3. Push the branch
4. Reply to the comment thread with the commit SHA:
   ```bash
   gh api repos/winkoz/plonk/pulls/<pr-number>/comments/<comment-id>/replies -X POST -f body='Fixed in <commit-sha>'
   ```

### Phase 4: Reply to Declined Comments

For each comment marked as **decline**:

1. Ask the user for their reasoning (or draft a response for approval)
2. Reply to the comment thread:
   ```bash
   gh api repos/winkoz/plonk/pulls/<pr-number>/comments/<comment-id>/replies -X POST -f body='<reasoning>'
   ```

### Phase 5: Summary

After all comments are addressed, present a summary:
- Number of comments fixed (with commit SHAs)
- Number of comments declined
- Any remaining unaddressed comments

### Rules

- Never reply to a comment without explicit user approval
- Each fix should be its own commit with a clear message
- Use `gh api .../comments/{id}/replies` for inline thread replies, not `gh pr comment`
- For general PR comments (not tied to code lines), use `gh pr comment <pr-number> --body "message"`
- Always push after committing fixes so the reply references a pushed commit
