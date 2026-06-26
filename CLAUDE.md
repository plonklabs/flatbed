# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

**IMPORTANT**: Never claim something is "tracked" or "documented" without actually doing it in the same response. If you say "tracked on #X" or "documented in Y", the issue/file MUST be updated before you finish your response. Making false claims about tracking is unacceptable.

## Minimalism + pragmatism

> *"Lo que no sirve que no estorbe"* — what doesn't serve a purpose shouldn't be in the way.

Keep the codebase (code, docs, scripts, configs, tests, CI surface) as **minimal as possible while being as pragmatic as possible**. The two halves are equally weighted: aggressive deletion of dead weight, but not at the cost of breaking real workflows, losing genuine context, or shipping cute-but-fragile abstractions.

Apply to:

- **Code.** Dead modules, unused helpers, dead `_prior` params, compatibility shims for migrations that already completed, "future-proofing" abstractions for hypothetical needs — delete. Three similar lines beats a premature trait. (See the "Doing tasks" rules: don't design for hypothetical future requirements.)
- **Docs.** Historical / superseded docs rot fastest of all (every line is a claim about code that has since moved). If a doc is labeled "historical" or "legacy" or "preserved for context," ask whether `git log` doesn't already provide that — if yes, delete the doc and strip its inbound links. The decision rule: would a new contributor reading the repo today benefit from this doc, or would they be confused by it? Confusion → delete.
- **Comments.** Already covered under the "Comments" section below and in `docs/style.md`. Default to not writing them; when one earns its place it states a property of the world the code runs in, not a property of the change that introduced it.
- **Tests.** Skip the test for the speculative case nobody will ever hit; keep the test for the regression that actually happened.
- **CI / scripts / Makefile.** Targets that nobody runs, mirror entries for images nothing consumes, env-var knobs with no remaining caller — delete.

**Pragmatism guardrails** (when *not* to delete):

- A doc / helper / Make target is still load-bearing for a real workflow even if it looks dusty — verify before deleting.
- A migration is mid-flight (some callers cut over, others not yet) — finish the migration before deleting the shim, not in parallel with it.
- A test exercises a regression we shipped — even if the surrounding feature is "stable now," the test is the receipt and stays.

When in doubt, delete and put the rationale in the commit message. If the deletion turns out to have been wrong, `git revert` is one command. The opposite mistake — letting rot accumulate — has no symmetric undo.

## Project Overview

**Plonk** is a Kubernetes infrastructure management platform written in Rust that provides unified secret management and service mesh control within Kubernetes clusters. It is part of the larger Winkoz Group monorepo.

The project consists of:
- **plonk_cli**: Interactive TUI-based CLI for infrastructure initialization and management
- **plonk_operator**: Kubernetes operator for reconciliation and telemetry
- **plonk_crds**: Custom Resource Definitions library for Kubernetes resources
- **plonk_gateway**: API gateway service (early stage)

## PR Workflow (CRITICAL - READ FIRST)

**IMPORTANT**: Never mention AI tools, Claude, or add Co-Authored-By tags in commits or PRs. All work should appear as standard developer contributions.

Every task should follow this workflow:
1. **Start from main**: If working in a worktree, use `git fetch origin main && git switch -c feature/your-feature-name origin/main`. Otherwise `git checkout main && git pull && git rebase origin/main && git switch -c feature/your-feature-name`
2. **Create feature branch**: (included in step 1 above)
3. **Develop**: Implement changes, write tests, run lints
4. **Test automation**: Create automated tests for your changes
5. **Clean up**: Format code, remove debug statements, update comments
6. **Create draft PR**: Always create PRs as drafts (`gh pr create --draft`). Only the user moves them to "ready for review" — never do this automatically.
7. **Update PR description**: Keep PR description current with all changes
8. **Address PR comments**: See "Addressing PR Review Comments" section below
9. **Keep PR updated**: Continue updating description as you make fixes
10. **Finalize**: User marks PR ready for review, then merges

Track tasks, decisions, blockers, and progress in the GitHub Issue linked to your PR.

## Addressing PR Review Comments

Use `/review <pr-number>` for the interactive workflow. The key principles:

- **Fetch** review comments via `gh api repos/plonklabs/plonk/pulls/{n}/comments`
- **Fix or decline** each comment — each fix gets its own commit
- **Reply inline** using `gh api .../comments/{id}/replies` with the commit SHA (if fixed) or reasoning (if declined)
- **General PR comments** (not in a thread): use `gh pr comment {n} --body "message"`

## Build and Development Commands

```bash
# Build and run
make build-plonk-cli  # or: cargo run -p plonk_cli
cargo build -p plonk_cli --release
cargo build -p plonk_operator --release
cargo build --workspace

# Test and quality
cargo test --workspace
cargo test -p plonk_cli
cargo clippy --workspace --all-targets --all-features
cargo fmt --all
```

## End-to-End Testing

**CRITICAL — production cluster guardrail.** The `plonk-production`
kubectl context (and any other non-`k3d-*` context) points at a real
production Kubernetes cluster. **It must never be the target of any
e2e test, `plonk install`, `plonk uninstall`, or destructive `kubectl`
command run from this repo.** Trusting `KUBECONFIG` isolation alone is
not sufficient — env-var plumbing has silently fallen back to
`~/.kube/config` before and trashed production.

**Invariant:** every destructive Make target — anything that calls
`plonk install`, `plonk uninstall`, `kubectl delete`, `kubectl rollout
restart`, or any other cluster-mutating command on an *existing*
cluster — MUST call `scripts/assert-kubectl-context.sh
<expected-context>` before that destructive operation. A target that
must first create its own k3d cluster (e.g. `e2e-full`) may run the
creation step before the assertion: a new k3d cluster with a
deterministic name cannot accidentally land on an existing production
cluster, so the creation itself needs no guard. Targets that create
their own cluster also need a target-scoped `export KUBECONFIG =
<slot-local file>` so the new context is written to an isolated
file, not `~/.kube/config`.

**CRITICAL**: Always use the Plonk CLI for E2E testing. Never manually apply YAML files or use kubectl directly for installation/uninstallation.

E2E has two entry points, depending on whether you want a hermetic
throwaway cluster or to reuse your worktree's persistent dev cluster:

| Target | Cluster | When |
|---|---|---|
| `make e2e-worktree` | `plonk-$PLONK_SLOT` (your worktree's dev cluster) | Day-to-day iteration. Tears Plonk down first, reinstalls fresh, leaves the cluster up afterwards. |
| `make e2e-full` | `plonk-e2e` (created + destroyed) | Hermetic one-shot. What CI runs. Use locally when you want a clean-room repro that's guaranteed not to be contaminated by prior dev state. |

Both produce identical test runs — the tests are kubeconfig-agnostic
and just hit whatever context is active.

### Worktree workflow (daily driver)

```bash
# From a worktree (e.g. worktrees/plonk1):
make e2e-worktree
```

That target chains: `plonk uninstall`, reap leftover `e2e-*`
namespaces from prior runs, build + import all four fixture images
at the slot tag `:e2e-test-plonk$N`, `plonk install` with those
images, wait for readiness, then `cargo test -p plonk_operator
--test e2e -- --ignored`. The cluster stays running so the next
invocation skips the create step.

**Concurrent worktrees.** Two worktrees can run `make e2e-worktree`
simultaneously — their cluster-targeting operations don't collide:

- The k3d cluster name is slot-aware (`plonk-$PLONK_SLOT`).
- The Docker image tag is slot-aware (`e2e-test-plonk$PLONK_SLOT`).
- Cluster-targeting kubectl/plonk calls in the recipe read and
  write a slot-local kubeconfig
  (`/tmp/plonk-kubeconfig-plonk$PLONK_SLOT.yaml`) via the
  target-scoped `KUBECONFIG` export — they never touch
  `~/.kube/config`. After the slot file is written, the recipe
  has a separate explicit step that `kubectl config view
  --flatten`s the slot context into `~/.kube/config` so it shows
  up alongside the user's other contexts (`kubectl config
  get-contexts`). The flatten lists the slot file FIRST in
  `KUBECONFIG` so its `cluster`/`user`/`context` entries
  override any stale `k3d-plonk-N` entry already in
  `~/.kube/config` (after a `make dev-cluster-delete` + recreate
  the slot file carries the fresh CA / server cert, and a
  `~/.kube/config`-first ordering would keep the broken old
  entry). To prevent the slot's `current-context` from silently
  winning under that ordering, the recipe captures whatever
  `~/.kube/config` had as `current-context` before the flatten
  and re-applies it on the merged file via `kubectl config
  use-context` — so production never silently flips to the
  worktree. If the user has no `current-context` set (or the file
  is freshly touched), the slot's `current-context` wins by
  default, which is harmless because there's no production
  context to protect. The `assert-kubectl-context.sh` guard reads
  from the slot file (the target-scoped export), not from
  `~/.kube/config`, so the recipe's own context guard is
  unaffected by the merge. (Interactive `make dev` keeps its
  existing UX of merging into the global file via `k3d kubeconfig
  merge`.)

The merge step is the one place this recipe writes shared global
state (`~/.kube/config`). It uses a slot-scoped temp file inside
`$HOME/.kube/` for the intermediate `kubectl config view
--flatten` output. Same-filesystem placement matters: on Linux
`/tmp` is usually tmpfs while `$HOME` lives on the main disk, and
a cross-filesystem `mv` falls back to copy+unlink — not atomic,
so a concurrent writer could see a half-written file. Keeping the
temp file under `$HOME/.kube/` keeps `rename(2)` in-kernel and
atomic, so each writer's payload lands whole. The redirect runs
under `umask 177` so the temp file is born at mode 0600 — without
it the temp is created world-readable (0644) and the post-`mv`
`~/.kube/config` inherits that mode until the subsequent `chmod
600`, briefly exposing tokens and client certs to other local
users. Under truly concurrent runs, both slots read
`~/.kube/config` before either writes; each flattens `(own_slot +
existing)` independently and races to `mv`. The final rename is
last-writer-wins — the losing slot's cluster/user/context entries
may be absent from `~/.kube/config` until that slot re-runs
`e2e-worktree`. The file itself is never half-written.

If you need to re-run after a code change in the operator, just
re-run `make e2e-worktree` — the image build is layer-cached so
operator-only changes take seconds.

### Hermetic workflow

```bash
make e2e-full
```

That target creates `plonk-e2e` (separate from any worktree dev
cluster), runs the whole flow, and deletes the cluster on the way out.
Mirrors what `.github/workflows/e2e.yml` does in CI.

### What the fixtures are

E2E needs four images in whichever cluster is in play:

- **operator** — Plonk's control plane.
- **rocket** — the Plonk-conformant test fixture used as the `PlonkBox` image; serves `/healthz`, `/readyz`, `/metrics` on 8080 (`nginx` does not, and `PlonkBox` probes hang on readiness if you point them at it).
- **mesh-rocket** — active-caller fixture for cross-PlonkBox tests.
- **plonk-proxy** — Plonk's sidecar Envoy image (`FROM envoyproxy/envoy:v1.32.5` + `curl` for the kubelet exec readiness probe).

To target a specific cluster manually (instead of via the
`e2e-worktree` / `e2e-full` chains):

```bash
# Into the worktree's dev cluster (auto-detected from $PWD):
make dev-load-images
make dev-load-image          # operator only
make dev-load-rocket         # rocket
make dev-load-mesh-rocket    # mesh-rocket
make dev-load-proxy          # plonk-proxy

# Into the hermetic plonk-e2e cluster:
make e2e-load-images

# Clean rebuild (skip Docker layer cache):
make dev-load-images DOCKER_BUILD_FLAGS=--no-cache
```

See `plonk/apps/rocket/README.md` for rocket's env-var knobs
(`ROCKET_READY_DELAY_SECS`, `ROCKET_METRICS`) — both are useful when
writing transition-style e2e tests.

### Verifying a deployment by hand

```bash
# Check pods
kubectl get pods -n plonk

# Check operator logs
kubectl logs -l app.kubernetes.io/name=plonk-operator -n plonk --tail=50
```

### Testing New Features

When testing operator features (like namespace RBAC):

1. **Create test resources:**
   ```bash
   # Create managed namespace
   kubectl create namespace test-managed
   kubectl label namespace test-managed plonk.tools/managed=true

   # Create unmanaged namespace for negative testing
   kubectl create namespace test-unmanaged
   ```

2. **Deploy test CRDs:**
   ```bash
   kubectl apply -f - <<EOF
   apiVersion: plonk.tools/v1
   kind: PlonkBox
   metadata:
     name: test-app
     namespace: test-managed
   spec:
     # Use the rocket test fixture — it satisfies the platform contract
     # (/healthz, /readyz, /metrics on admin_port). nginx and similar
     # arbitrary images will hang on readiness. The tag below is the
     # default (`e2e-test`, loaded by `make e2e-full`). Under
     # `make e2e-worktree` the tag is `e2e-test-plonk$N` — substitute
     # the slot-aware tag if `rocket:e2e-test` is not in your cluster.
     image: rocket:e2e-test
     min_replicas: 1
     max_replicas: 2
   EOF
   ```

3. **Verify expected behavior:**
   ```bash
   # Check logs for processing
   kubectl logs -l app.kubernetes.io/name=plonk-operator -n plonk --tail=30

   # Verify resources created
   kubectl get deployment,pods -n test-managed
   ```

### Common Issues

**Image not updating:**
- Use a unique tag (e.g., timestamp) or rebuild with `DOCKER_BUILD_FLAGS=--no-cache`
- Delete the pod to force recreation: `kubectl delete pod -l app=plonk-operator -n plonk`

**Permission errors:**
- RBAC changes require pod restart to pick up new permissions
- Reinstall using CLI to ensure ClusterRole/ClusterRoleBinding are updated

**CLI requires TTY:**
- Always use `--yes` flag for non-interactive mode
- Example: `plonk install --yes --namespace plonk`

## Coding Style Guidelines

**IMPORTANT**: Follow the coding standards defined in `docs/style.md` for all code changes.

Key guidelines:
- **Favor let-else over nesting**: Use `let-else` patterns with early returns instead of deeply nested `if-let` blocks
- **Keep functions flat**: Maximum 1-2 levels of indentation
- **Early returns**: Return early on error conditions and guards
- **Use ? operator**: Prefer `?` for error propagation over explicit match
- **No wildcard imports**: Never use `use foo::*`; always import specific items explicitly
- **Document public APIs**: All public functions, types, and modules need doc comments
- **Run clippy and fmt**: Before committing, run `cargo clippy` and `cargo fmt`

See `docs/style.md` for complete guidelines with examples.

### Comments

Comments rot. Unlike code, they're verified by nothing — no compiler,
no test, no reviewer can confirm a comment still matches the code it
sits next to. Every comment is a quiet bet that whoever changes the
surrounding code next will also update the comment. That bet is usually
lost: the code drifts, the comment stays, and the next reader trusts a
sentence that has become a lie.

So the default position is **don't comment**. Names, types, and control
flow are the primary documentation. A comment has to earn its place by
carrying information that:

1. **Cannot be inferred from the surrounding code or types.** If reading
   the line tells the reader what it does, a comment that restates it is
   pure overhead.
2. **Will be useful to a future reader,** not just to the author at write
   time. Six-months-later you doesn't have today-you's context.
3. **Will remain accurate through reasonable refactors.** A comment that
   any nearby restructure could quietly invalidate is a future lie
   waiting to happen.

#### When a comment earns its place

The shapes below recur and are worth the cost:

- **Constraints from outside the code.** A platform behaviour, library
  quirk, or wire-format requirement that the types can't express.
  ```rust
  // Image pulls run in the kubelet's host network namespace, where
  // cluster DNS doesn't resolve. The rewrite step substitutes the
  // cluster IP so the ref the kubelet sees is actually reachable.
  ```

- **Invariants the caller is responsible for.** Contracts the type
  system can't enforce, made explicit so a future caller doesn't break
  the function by accident.
  ```rust
  // Caller holds the write lock — the cursor advance below isn't
  // safe under concurrent reads.
  ```

- **Surprising library or language behaviour.** A standard-library or
  external API that doesn't behave the obvious way; the comment names
  the foot-gun.
  ```rust
  // `Option::is_none_or` short-circuits on `None` before evaluating
  // the closure — `is_some_and(!...)` would evaluate the closure on
  // a `None` and panic on the deref inside.
  ```

- **Workarounds for a bug, named by its property.** Not by issue number
  (which rots), but by the bounded condition under which the workaround
  is load-bearing.
  ```rust
  // `serde_yaml` rejects integer keys for a `String`-keyed map even
  // when the integer would coerce cleanly. Read as `serde_json::Value`
  // first and stringify keys before re-deserialising.
  ```

In every case the comment describes a *property of the world the code
runs in*, not a property of the change that introduced it. That's what
makes it durable.

#### Patterns that rot the fastest

The shapes below turn into lies first. Don't write them; delete them
when you find them.

- **Restating what the code already says.** `// increment the counter`
  next to `i += 1`. The reader spends attention parsing both, with
  nothing gained.

- **Forward-looking or temporal phrasing.** "Future X will…", "for when
  we add Y", "the new path", "the pre-cutover overlap". Wrong the
  moment X is renamed, Y never ships, "new" becomes the default, or
  the cutover completes. State the property without dating it.

- **References to other code, docs, or issues by path or name.**
  `// see foo::bar for the equivalent`, `// matches lib/other.rs`,
  `// PR #123 spells out why`. The referenced thing keeps compiling
  fine when it's renamed or moved; the comment silently points at
  nothing. State the property the reference would have explained,
  inline.

- **Reasoning about the patch instead of the code.** "Noting this for
  future readers so they know it's intentional", "documenting the gap
  this change couldn't close". Reasoning about a *change* belongs in
  the PR description and commit message, which are frozen in time and
  searchable from `git log`. Source comments have to keep being
  accurate as the code keeps moving — a poor home for any thought
  scoped to "the state of the world when I wrote this".

- **Author-mental-state asides.** "Tricky", "TODO: probably refactor",
  "not sure why this works". Either fix what the comment is hedging
  about, or replace the hedge with a specific statement about the
  invariant.

#### The self-test

Before keeping or writing a comment, walk it through three questions:

1. **Removal test.** If I deleted this comment, would a reader with the
   surrounding code and a working knowledge of the codebase actually be
   confused? If no, the comment is noise.
2. **Refactor test.** Could a sensible nearby restructure leave the
   comment quietly wrong without anyone noticing? If yes, restructure
   the code so the comment isn't needed, or delete the comment.
3. **Scope test.** Is the comment about the *code as it stands*, or
   about the *change that produced it*? Code-as-it-stands belongs in
   the source; change-reasoning belongs in the PR description and
   commit message.

The three apply to module docs, function docs, inline `//` comments,
test comments, and TODOs the same way. When self-reviewing a diff,
walk every comment in it through the three questions before pushing —
most of what gets caught downstream is caught here for free.

## Flatbed Framework

The project uses the internal `flatbed` framework for HTTP services with FlatBuffers support.

### Worker and Route Discovery

The `#[worker]` and `#[route]` macros use the `inventory` crate for **compile-time registration**:

- **No re-exports needed**: Simply declaring a module (`mod workers;`) is sufficient. The macros register items automatically via `inventory::submit!`
- **Don't suppress unused import warnings**: If the compiler says a re-export is unused, it's genuinely unused. The inventory system discovers items regardless of module visibility
- Workers are spawned automatically by `Flatbed::run()`

```rust
// workers/mod.rs - correct pattern
mod deploy;  // Just declare, no pub use needed

// workers/deploy.rs
#[worker(name = "my-worker", description = "Does work")]
pub async fn my_worker(ctx: Arc<AppContext>) -> Result<(), FlatbedWorkerError> {
    // Worker logic
}
```

## Feature Planning with GitHub Issues

Features and design work are tracked using **GitHub Issues** as epics with checkbox lists. Use `/spec` to create feature specs interactively.

### Structure

- **Epic Issue**: Full feature design (context, proposal, technical design, changes required, dependencies) with a checkbox list of implementation steps
- **Standalone Issues**: For compiler warnings, tech debt, or small fixes that don't belong to an epic

Do **not** create sub-issues. All steps live as checkboxes in the epic body to avoid notification spam and keep tracking in one place.

### Workflow

1. Pick a step from an epic's checkbox list
2. Create a feature branch: `git switch -c feature/step-description`
3. Implement the changes described in the step
4. Open a PR referencing the epic: `Part of #<epic-number>`
5. After merge, check off the step in the epic body
6. When all steps are checked, close the epic

## Key Architecture Notes

### Resource Construction Pattern

The operator builds Kubernetes resources in two ways:

- **Plonk-owned resources** (Deployment, Service, Secret): Built with typed k8s-openapi structs in worker modules (e.g., `build_deployment()` in `plonk_box_deploy.rs`)
- **Third-party CRD resources** (ARC AutoscalingRunnerSet, future CloudNativePG, Redis): Built programmatically with `serde_json::json!()` via renderer modules in `services/renderers/`

Renderers take a typed params struct and return `serde_json::Value`. Vendored Helm chart YAML in `manifests/` is a checked-in reference for version tracking -- NOT used at runtime.

See `docs/architecture.md` for the full renderer pattern documentation.

### Modifying CRD status fields

Status patches use `kube::api::Patch::Merge` (JSON Merge Patch, RFC 7396). Two consequences trip people up:

1. **`Option<T>` + `#[serde(skip_serializing_if = "Option::is_none")]` is a "preserve etcd value" signal, not a "clear" signal.** When the field is `None`, the merge-patch omits the key, so the API server keeps whatever is stored. `None` means "I have nothing to write; trust the existing value."
2. **Array fields like `conditions: Vec<Condition>` are replaced wholesale.** RFC 7396 has no merge-key semantics for arrays, so any reconciler patching the array must own the entire contents.

Before editing a status struct or its reconciler, audit the struct by wire shape:

| Wire shape | Treatment | Goes into "should I patch?" comparison? |
|---|---|---|
| `T` written every reconcile | actively driven by this reconciler | **YES** — straight equality |
| `Vec<T>` + `skip_if_empty` written every reconcile | actively driven; sole writer required | **YES** — straight equality |
| `Option<T>` + `skip_if_none`, this reconciler writes `Some` on some branches and `None` on others | preserve via skip-serialize on the off branches | **CONDITIONALLY** — `proposed.is_some() && proposed != existing`. Comparing straight `!=` causes no-op patch loops on transitions (proposed `None` vs etcd `Some`); omitting entirely causes missed re-population if etcd ever drifts to `None` while we hold a `Some` |
| `Option<T>` + `skip_if_none`, owned by a different worker | always pass `None`; never copy the snapshot value back | **NO** — copying back creates a TOCTOU window against the real owner |

When extracting a "did anything change?" helper, name it for what it actually answers (e.g. `deploy_status_changed` compares only fields the deploy reconciler actively drives) and put the audit in the doc-comment.

**Class-level fixes, not instance-level.** When a review flags a regression on field X with this shape, grep the module for siblings with the same shape and apply the fix uniformly — the bug almost always recurs on every field that matches the pattern. PR #200 shipped two rounds of the same regression on different fields because the first fix was instance-level. See `plonk/apps/operator/src/workers/deploy.rs` `proposed_status` / `deploy_status_changed` for the worked example.

**Test transitions, not snapshots.** Status-machine bugs hide in transitions (Ready → invalid spec, Ready → Deployment-deleted, cert_issuer-write between deploy snapshots). Pin the comparisons across transitions, not just steady-state serialisation.

### Reconciler shape

The operator's target shape for every CR is a **reconciler that decides
+ observes + acts**: it reads CR + downstream state, decides each
owned condition's state for the current generation, and calls
in-process service functions to drive Kubernetes resources. Every
reconciler is `register_kube_native_reconciler!` — no message bus
between decide and act.

**Reconciler responsibilities:**

- Read CR + downstream resources (StatefulSet / Deployment / PVC /
  Service / Job).
- Decide each owned condition's state for the current generation.
- Call `services::<crd>::install` / `uninstall` for side-effects.
- SSA-apply conditions with the reconciler's `fieldManager`.
- Requeue: short interval (e.g. 15s) while installing, long interval
  (e.g. 60s) when Ready.

**Service responsibilities:**

- Render and SSA-apply downstream resources.
- Optionally SSA-apply a cursor condition under a distinct
  `fieldManager` (e.g. `ResourcesApplied=True` under the installer
  fieldManager) — the cursor lets the reconciler observe in the
  next cycle whether the previous install ran and at what
  generation.
- Return `Result<(), kube::Error>` synchronously.

**Multi-writer SSA on conditions.** Plonk has **two coexisting
patterns** for writing `.status.conditions[]`:

- **Aggregator path** (PlonkBox, PlonkNamespace): publishers enqueue
  state-change events on an in-process `tokio::sync::mpsc` channel
  (`ctx.status_tx`); one leader-gated `plonk-status-aggregator`
  worker drains the channel, holds the truth, and is the sole
  writer of the conditions array. Documented in
  [`docs/architecture.md`](docs/architecture.md) under
  "Status Writes and Field-Manager Hygiene". New work on these CRs
  must keep using this path — direct condition writes from any
  fieldManager other than `plonk-status-aggregator` would un-claim
  the aggregator's entries on every reconcile.
- **Direct multi-writer SSA** (PlonkDomain, PlonkRDS, PlonkRDSUser,
  PlonkStorage, PlonkRegistry, PlonkRunnerSet, PlonkGateway,
  PlonkRoute): each writer holds a distinct `fieldManager` and
  Applies its own cursor condition directly to
  `.status.conditions[]`. The SSA list-map merge keys on `type`,
  so per-writer entries stay independent without an aggregator
  collapse. This section's rules below apply to **this** pattern.

The decision rule for a new CR: if the new CR introduces new
condition types that don't exist on PlonkBox / PlonkNamespace and
the reconciler/service split here applies, use the direct multi-
writer SSA pattern. Adding a new condition type to PlonkBox or
PlonkNamespace stays on the aggregator path.

Rules for the direct multi-writer pattern — each writer must:

- Use a distinct `fieldManager` (e.g. `plonk-rds-reconciler` vs
  `plonk-rds-installer`). The fieldManager is the field-ownership
  key; two writers sharing one fieldManager will clobber each other.
- Apply with `.force()` so transient ownership conflicts (race during
  a coordinated migration) don't fail the patch.
- Use the `conditions_schema` helper in `plonk_crds::crds` to attach
  the `x-kubernetes-list-type=map` + `list-map-keys=[type]` markers
  — without these markers the apiserver treats the array as
  monolithic and the multi-writer story silently breaks.
- Sort by `type` before comparing in tests so the "did anything
  change?" comparison is order-insensitive.

**No `phase` enum on new CRs.** Use the aggregate `Ready` condition.
The `phase` enum survives on PlonkRunnerSet and PlonkNamespace as a
not-yet-migrated legacy field — both are tracked in the operator
refactor epic #367. Every other CR (PlonkBox, PlonkRegistry,
PlonkStorage, PlonkGateway, PlonkRoute, PlonkDomain) already ships
conditions-only. New CRs ship in the target shape directly.

**Reason vocabulary as pinned constants.** Every `Reason` string a
condition writer can produce is a `pub const` in `plonk_crds::crds`
(e.g. `RDS_REASON_TIER_NOT_FOUND`). Producer (reconciler / service)
and consumer (CLI's install wait, dashboards, alerts) reference the
constants by name — never inline literals — and a single test pins
the constant list as stable camel-case so drift is caught at compile
time.

