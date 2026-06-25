# /pr — Create or Update a Pull Request

## Description
Creates a PR with a structured description that lets a reader understand the change without reading the diff. Can also update an existing PR's description.

The canonical example for this project is [PR #364](https://github.com/plonklabs/plonk/pull/364) (PlonkStorage slice 5 — PlonkNats CRD + conditions-based reconciler/worker). When in doubt about structure, tone, or how to weave code blocks and mermaid diagrams into the body, open that PR and match it.

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
2–4 sentences. State what the PR does and the headline test signal (e.g. `make e2e-full → 39/39 green in 9m`). Link the epic it's part of. If it lands a methodology shift (new architectural pattern, refactor target), name it here and link the doc that codifies it.

#### Test plan
Markdown checklist. Mark completed items with `[x]`. List every verification command actually run, with results:
- `cargo fmt --all`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p <package>` with pass count
- `make e2e-full` with pass count + wall-clock when relevant
- `make check-docs` when docs were touched
- Manual verifications and the commands that produced them

Change-shape sections (include the ones that apply, in this order):

#### Wire-format diff
Whenever the PR changes a CRD spec/status, a FlatBuffer message, an env var contract, or any other persisted interface, show the **before → after** as YAML/Rust/proto snippets in a fenced code block. The reader should see the shape change without inferring it from the diff.

#### Architecture
Explain how the new pieces fit together. For reconciler/worker work this means: which fieldManager owns which condition, how the watch fires, what the worker's exact responsibility is. Include a **mermaid sequence diagram** when the flow crosses ≥3 actors (CR → reconciler → NATS → worker → apiserver → kube-controller). Validate every mermaid block with `make check-docs` (or `mmdc`) before pushing — broken diagrams render as raw text on GitHub.

When this section is non-trivial, put the full version in `docs/<feature>.md` and link it from the PR; the PR section is the precis.

#### CLI flow
If `plonk install` / `plonk uninstall` ordering changed, list the new steps with the prior order alongside (numbered list or table). Call out steps that exist purely to dodge a chicken-and-egg (e.g. bootstrapping NATS with the operator's `fieldManager` so the first operator reconcile is a no-op SSA).

#### Storage layout / mount paths
If persistent storage shape changed (new PVC, new mount path, new tier consumer), state the path convention applied (`<tier-base>/<workload>/<n>` for StatefulSets, `<tier-base>/<workload>` for Deployments) and how the ordinal subdir is realised (downward API → shell wrapper).

#### HA semantics
For workloads that scale: explain the `replicas: 1` behaviour and the `replicas: N` behaviour. State the failure mode tolerated (e.g. "JetStream streams configured `replicas: 3` tolerate single-node failure"). Name any production-only knobs the change unlocks.

#### What this slice does NOT ship
Bullet list of related work explicitly out of scope, each pointing at the issue/epic that tracks it. The point is to head off "why didn't you also fix X?" review comments by showing that X is known and queued.

#### Spec-change re-dispatch (reconciler PRs only)
When the reconciler decides whether to re-publish a worker task, name the helper (e.g. `dispatched_for_generation`) and the comparison it does (`Dispatched.observedGeneration` vs `metadata.generation`). Link the unit test that pins boundary behaviour.

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

PRs are always created as **draft** ([[feedback_draft_prs]]). The user marks ready for review; never do it automatically.

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

- **Title**: Under 70 characters. Lead with a verb or the epic/slice name (`PlonkStorage slice 5: ...`). No period.
- **Body**: Use a tempfile + `--body-file`, never inline heredoc with backticks.
- **Tone**: Concrete and specific. Code blocks beat prose summaries — show the actual YAML/Rust diff instead of describing it ([[feedback_human_communication]]).
- **Mermaid diagrams**: Validate with `make check-docs` before pushing. Avoid semicolons inside `Note over` text — the parser breaks.
- **No AI references**: Never mention AI tools, Claude, or add Co-Authored-By tags.
- **No fabricated estimates**: Don't add "rollout takes ~2h" or similar timings unless they were actually measured ([[feedback_no_fabricated_estimates]]).
- **No forward-looking phrasing in inline comments** the PR introduces: keep PR-anchored TODOs out of source per the CLAUDE.md comment-hygiene rules.
- Always show the user the drafted title and description before creating/updating.
- Return the PR URL when done.
