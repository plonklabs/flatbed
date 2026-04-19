# /spec — Create a Feature Spec with GitHub Issues

## Description
Guided workflow for designing a feature spec and creating a GitHub Issue (epic) to track implementation.

## Instructions

When the user runs `/spec <feature-name-or-description>`, follow this workflow:

### Phase 1: Gather Information

Walk the user through each section of the spec. Ask one section at a time, offering to help draft content. The sections are:

1. **Context** — What problem does this solve? What is the current state?
2. **Proposal** — High-level solution overview (1-2 paragraphs)
3. **User Flows** — Concrete scenarios showing how users interact with this feature end-to-end. Each flow should be a numbered sequence of steps from the user's perspective.
4. **Acceptance Criteria** — Checkbox list of observable, testable outcomes that define "done". These should be verifiable without reading the code.
5. **Design** — Detailed technical design: types, traits, code flow, code examples where helpful
6. **Changes Required** — List of files/modules affected with specific modifications
7. **Dependencies** — External crates, services, assumptions, or prerequisites

For each section, present what you have so far and ask the user to confirm or revise before moving on.

### Phase 2: Break Down into Steps

Once the spec is complete:

1. Propose a set of implementation steps as a **checkbox list** inside the epic issue body
2. Each step should map to roughly one PR and have a clear imperative description
3. Include enough detail in each step that someone picking it up knows what to do
4. Ask the user to confirm, reorder, split, or merge steps

**Do NOT create sub-issues.** All steps live as checkboxes in the epic body. This avoids notification spam and keeps tracking in one place.

### Phase 3: Review & Create

1. Present the **full epic issue** for final review
2. Only after the user explicitly approves, create it using `gh issue create`
3. Apply the `📦 epic` label and any relevant component labels

### Issue Format

**Epic issue:**
```
## Context
...

## Proposal
...

## User Flows

**<Persona> does <action>:**
1. Step from user's perspective
2. What happens next
3. Observable outcome

## Acceptance Criteria

- [ ] Testable outcome 1
- [ ] Testable outcome 2
- ...

## Design
...

## Changes Required
...

## Dependencies
...

## Steps
- [ ] **Step 1: <imperative description>** — detail on what this covers, which files change, verification
- [ ] **Step 2: <imperative description>** — ...
- ...
```

Each step checkbox should be self-contained: bold title, then a dash and enough context to work from. When a step is completed, check the box and reference the PR in a comment.

User flows define the "what" from the user's perspective. Acceptance criteria define the "done" with testable outcomes. Steps define the "how" from the developer's perspective.

### Rules

- Never create issues without explicit user approval
- Use imperative mood for step titles ("Add X", "Refactor Y", not "Added X")
- Keep steps small enough for a single PR (ideally reviewable in one sitting)
- Do NOT create separate sub-issues — use checkbox lists in the epic body
- One epic per feature area; fold related work into existing epics rather than creating new ones
