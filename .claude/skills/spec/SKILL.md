# /spec — Create a Feature Spec with GitHub Issues

## Description
Guided workflow for designing a feature spec and creating GitHub Issues (parent + sub-issues) to track implementation.

## Instructions

When the user runs `/spec <feature-name-or-description>`, follow this workflow:

### Phase 1: Gather Information

Walk the user through each section of the spec. Ask one section at a time, offering to help draft content. The sections are:

1. **Context** — What problem does this solve? What is the current state?
2. **Proposal** — High-level solution overview (1-2 paragraphs)
3. **Design** — Detailed technical design: types, traits, code flow, code examples where helpful
4. **Changes Required** — List of files/modules affected with specific modifications
5. **Dependencies** — External crates, services, assumptions, or prerequisites

For each section, present what you have so far and ask the user to confirm or revise before moving on.

### Phase 2: Break Down into Sub-Issues

Once the spec is complete:

1. Propose a set of implementation steps (sub-issues), where each step maps to exactly **one PR**
2. Each sub-issue should have:
   - A clear title: `Step N: <imperative description>`
   - A body with: Goal, Changes (file-by-file), and Verification steps
3. Ask the user to confirm, reorder, split, or merge steps

### Phase 3: Review & Create

1. Present the **full parent issue** and **all sub-issues** for final review
2. Only after the user explicitly approves, create them using `gh issue create`
3. Create the parent issue first, then create each sub-issue referencing the parent
4. Link sub-issues to the parent using GitHub's sub-issue feature or body references

### Issue Format

**Parent issue:**
```
## Context
...

## Proposal
...

## Design
...

## Changes Required
...

## Dependencies
...

## Implementation Steps
- [ ] #<sub-issue-1>
- [ ] #<sub-issue-2>
- ...
```

**Sub-issue:**
```
Parent: #<parent-issue-number>

## Goal
What this step accomplishes.

## Changes
File-by-file modifications with code examples.

## Verification
Build, test, and validation commands.
```

### Rules

- Never create issues without explicit user approval
- Keep sub-issues small enough for a single PR (ideally reviewable in one sitting)
- Use imperative mood for issue titles ("Add X", "Refactor Y", not "Added X")
- Reference the parent issue number in every sub-issue body
