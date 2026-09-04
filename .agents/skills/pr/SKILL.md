---
name: pr
description: Create or update a pull request with a complete description and linked issue.
---

# /pr — Create or Update a Pull Request

## Description
Creates a PR with a structured description that lets a reader understand the change without reading the diff. Can also update an existing PR's description.

When in doubt about structure, tone, or how to weave code blocks and mermaid diagrams into the body, read a recent well-formed PR in this repository and match it.

## Instructions

When the user runs `/pr` or `/pr <pr-number>`, follow this workflow:

### Phase 1: Gather context

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

### Phase 2: Draft the description

The description has a **fixed skeleton** plus **change-shape sections** that you include only when the change actually has them. The skeleton always present:

#### Summary
2–4 sentences. State what the PR does and the headline test signal (e.g. `cargo test --workspace → 214 passed`). Link the epic it's part of. If it lands a methodology shift (new architectural pattern, refactor target), name it here and link the doc that codifies it.

#### Test plan
Markdown checklist. Mark completed items with `[x]`. List every verification command actually run, with results:
- `cargo fmt --all`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace` (or `-p <package>`) with pass count
- `bash scripts/check-generated.sh` when schemas or codegen changed
- The broker-backed NATS suite when the worker layer changed, with pass count
- `npm run build` / client test runs from `clients/ts/` when the TS client changed
- Manual verifications (a running example + `curl`) and the commands that produced them

Change-shape sections (include the ones that apply, in this order):

#### Wire-format diff
Whenever the PR changes a `.fbs` schema, the generated Rust or TypeScript codecs, the HTTP error wire format, an env var contract, or any other persisted interface, show the **before → after** as fenced code snippets. The reader should see the shape change without inferring it from the diff.

#### Architecture
Explain how the new pieces fit together: which layer owns what (server, route registry, codec, worker), where a request or message enters and leaves, and what the macro-generated code contributes. Include a **mermaid sequence diagram** when the flow crosses ≥3 actors (client → server → handler → NATS → worker). Preview every mermaid block (e.g. `mmdc`, or the GitHub preview) before pushing — broken diagrams render as raw text.

When this section is non-trivial, put the full version in a doc under the relevant crate and link it from the PR; the PR section is the precis.

#### CLI / codegen flow
If the `flatbed` CLI's flags, inputs, or generation pipeline changed, show the before/after invocation and what the generated output now contains. Same for the TS client generator: name the inputs (`/openapi.json`, `/schema.bfbs`) and what changed in the emitted client.

#### Feature-gate matrix
If the change touches feature-gated code (`telemetry` / `openapi` / `nats` / `k8s`), state which feature combinations were built and tested — a change that compiles under `--all-features` can still break the default set, and vice versa.

#### What this PR does NOT ship
Bullet list of related work explicitly out of scope, each pointing at the issue/epic that tracks it. The point is to head off "why didn't you also fix X?" review comments by showing that X is known and queued.

### Phase 3: Create or update the PR

**For new PRs:**

Write the body to a tempfile and use `--body-file`. Heredoc + inline backticks in the body breaks GitHub's markdown rendering — backticks inside heredocs get escaped or mangled. Tempfile + `--body-file` is the only reliable form.

```bash
BODY_FILE=$(mktemp)
cat > "$BODY_FILE" <<'EOF'
## Summary
...
EOF

git push -u origin <branch-name>
gh pr create --draft --title "<title>" --body-file "$BODY_FILE"
rm "$BODY_FILE"
```

PRs are always created as **draft** — the draft window is the self-review stage; ready is flipped by the authorized `/implement` flow or the user, never as a side effect of creating.

**For existing PRs:**

```bash
BODY_FILE=$(mktemp)
cat > "$BODY_FILE" <<'EOF'
...
EOF
gh pr edit <number> --title "<title>" --body-file "$BODY_FILE"
rm "$BODY_FILE"
```

### Rules

- **Title**: Under 70 characters, imperative sentence case matching this repo's history (`Add broker-backed integration tests for the NATS worker layer`), no trailing period.
- **Body**: Use a tempfile + `--body-file`, never inline heredoc with backticks.
- **Tone**: Concrete and specific. Code blocks beat prose summaries — show the actual Rust/TS/schema diff instead of describing it.
- **Mermaid diagrams**: Preview before pushing. Avoid semicolons inside `Note over` text — the parser breaks.
- **No AI references**: Never mention AI tools or add Co-Authored-By tags.
- **No fabricated estimates**: Don't add timings or counts unless they were actually measured.
- **No forward-looking phrasing in inline comments** the PR introduces: keep PR-anchored TODOs out of source per the AGENTS.md comment rules.
- Always show the user the drafted title and description before creating/updating.
- Return the PR URL when done.
