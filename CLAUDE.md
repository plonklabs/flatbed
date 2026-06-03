# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

**IMPORTANT**: Never claim something is "tracked" or "documented" without actually doing it in the same response. If you say "tracked on #X" or "documented in Y", the issue/file MUST be updated before you finish your response. Making false claims about tracking is unacceptable.

## Minimalism + pragmatism

> *"Lo que no sirve que no estorbe"* — what doesn't serve a purpose shouldn't be in the way.

Keep the codebase (code, docs, scripts, configs, tests, CI surface) as **minimal as possible while being as pragmatic as possible**. The two halves are equally weighted: aggressive deletion of dead weight, but not at the cost of breaking real workflows, losing genuine context, or shipping cute-but-fragile abstractions.

Apply to:

- **Code.** Dead modules, unused helpers, dead `_prior` params, compatibility shims for migrations that already completed, "future-proofing" abstractions for hypothetical needs — delete. Three similar lines beats a premature trait. (See the "Doing tasks" rules: don't design for hypothetical future requirements.)
- **Docs.** Historical / superseded docs rot fastest of all (every line is a claim about code that has since moved). If a doc is labeled "historical" or "legacy" or "preserved for context," ask whether `git log` doesn't already provide that — if yes, delete the doc and strip its inbound links. The decision rule: would a new contributor reading the repo today benefit from this doc, or would they be confused by it? Confusion → delete.
- **Comments.** Already covered under "Comment hygiene" in `docs/style.md` and inline in this file — no forward-looking phrasing, no PR/issue/slice refs, no narrating what the code already says.
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

- **Fetch** review comments via `gh api repos/winkoz/plonk/pulls/{n}/comments`
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
| `make e2e-worktree` | `plonk-$PLONK_SLOT` (your worktree's dev cluster) | Day-to-day iteration. Tears Tilt and Plonk down first, reinstalls fresh, leaves the cluster up afterwards. |
| `make e2e-full` | `plonk-e2e` (created + destroyed) | Hermetic one-shot. What CI runs. Use locally when you want a clean-room repro that's guaranteed not to be contaminated by prior dev state. |

Both produce identical test runs — the tests are kubeconfig-agnostic
and just hit whatever context is active.

### Worktree workflow (daily driver)

```bash
# From a worktree (e.g. worktrees/plonk1):
make e2e-worktree
```

That target chains: kill any running `tilt up` for this slot, `tilt
down`, `plonk uninstall`, reap leftover `e2e-*` namespaces from prior
runs, build + import all four fixture images at the slot tag
`:e2e-test-plonk$N`, `plonk install` with those images, wait for
readiness, then `cargo test -p plonk_operator --test e2e --
--ignored`. The cluster stays running so the next invocation skips
the create step.

**Concurrent worktrees.** Two worktrees can run `make e2e-worktree`
simultaneously — none of the shared global state collides:

- The k3d cluster name is slot-aware (`plonk-$PLONK_SLOT`).
- The Docker image tag is slot-aware (`e2e-test-plonk$PLONK_SLOT`).
- The kubeconfig is written to a slot-local file
  (`/tmp/plonk-kubeconfig-plonk$PLONK_SLOT.yaml`) and exported as
  `KUBECONFIG` for every subprocess in the recipe; `~/.kube/config`
  is neither read nor written. (Interactive `make dev` keeps its
  existing UX of merging into the global file.)

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

### Comment hygiene

Comments rot faster than code. Two failure modes recur often enough that
the bot reviewer flags them on most PRs — head them off at the source.

**No forward-looking or speculative phrasing.** Avoid wording like
"future X tooling will…", "(none today — but added defensively so a
future Y…)", "when the cert worker grows a `CertReady` condition", "the
new NATS path", "the pre-cutover overlap". These claims become wrong the
moment X gets renamed, Y never ships, "new" becomes the default, or the
cutover completes. Describe the **property** (single source of truth,
contract, invariant) without naming speculative consumers or temporal
state:

```rust
// Bad — names a worker that may never write this and "future" rots
// once a foreign writer arrives (or never does).
/// Filters to deploy-owned condition types so foreign entries
/// (none today — but added defensively so a future foreign writer
/// never gets re-asserted under this manager).

// Good — describes the invariant; survives whether or not foreign
// writers ever appear.
/// Filters to deploy-owned condition types so anything that ever shares
/// the merged conditions list is never re-asserted under this manager.
```

**No slice/PR/issue references in source comments.** Phrases like
"PR #200 spelled out…", "the PR description spells out…", "slice 6b's
injector needs…", or bare `#123` rot the moment the PR/issue is closed.
PR descriptions live in PR history; epic context lives in issue bodies;
neither should be the load-bearing explanation for a line of code.
Inline the actual rationale instead:

```rust
// Bad
/// A refactor that accidentally aligns the two strings re-introduces
/// the clobbering regression the PR description spells out.

// Good
/// A refactor that accidentally aligns the two strings re-introduces
/// silent cross-manager clobbering on `/status` — the aggregator's
/// next Apply would un-claim the scalars this manager owns, and vice
/// versa, on every reconcile.
```

Both rules apply equally to module docs, function docs, inline `//`
comments, test comments, and PR-anchored TODOs. When self-reviewing,
grep the diff for `future `, `will `, `once a`, `PR #\d+`, `issue #\d+`,
and bare `#\d+` inside `///` or `//` lines before pushing.

## Flatty Framework

The project uses the internal `flatty` framework for HTTP services with FlatBuffers support.

### Worker and Route Discovery

The `#[worker]` and `#[route]` macros use the `inventory` crate for **compile-time registration**:

- **No re-exports needed**: Simply declaring a module (`mod workers;`) is sufficient. The macros register items automatically via `inventory::submit!`
- **Don't suppress unused import warnings**: If the compiler says a re-export is unused, it's genuinely unused. The inventory system discovers items regardless of module visibility
- Workers are spawned automatically by `Flatty::run()`

```rust
// workers/mod.rs - correct pattern
mod deploy;  // Just declare, no pub use needed

// workers/deploy.rs
#[worker(name = "my-worker", description = "Does work")]
pub async fn my_worker(ctx: Arc<AppContext>) -> Result<(), FlattyWorkerError> {
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

### Reconciler/worker split (conditions-based)

The operator's target shape for every CR is a **reconciler that decides
+ observes** and a **worker that does one simple, uninterrupted
action and acks**. The first complete realisation of this pattern is
PlonkNats — see [`docs/plonk_nats.md`](docs/plonk_nats.md) for the
worked example with mermaid diagrams. Match its structure when
introducing a new reconciler/worker pair, and migrate existing CRs
toward it (tracked in #367).

**Reconciler responsibilities:**

- Read CR + downstream resources (StatefulSet / Deployment / PVC /
  Service / Job).
- Decide each owned condition's state for the current generation.
- SSA-apply conditions with the reconciler's `fieldManager`.
- Publish a worker task when `Dispatched.observedGeneration` is stale
  vs `metadata.generation` (use the `dispatched_for_generation`
  helper or its equivalent).
- Requeue: short interval (e.g. 15s) while installing, long interval
  (e.g. 60s) when Ready.

**Worker responsibilities:**

- Validate the CR UID (refuses to act on a stale enqueued message
  whose CR has been recreated).
- Render and SSA-apply its owned resources.
- SSA-apply a single cursor condition (e.g. `ResourcesApplied=True`)
  with the worker's own `fieldManager`.
- ACK.

**What the worker MUST NOT do:**

- Block on `wait_for_*_ready`. Readiness is the reconciler's job;
  blocking in the worker conflates "did one action" with "downstream
  rolled out" and makes the system brittle to NATS redelivery.
- Patch `Ready` or any aggregate condition. Workers own only their
  cursor; the aggregate is the reconciler's role.
- Read its own SSA-applied resource back to verify. The cursor
  condition write IS the signal — the reconciler's kube watch on the
  CR fires when that condition lands and picks up from there.

**Multiple actions per reconciler.** A reconciler can drive more
than one worker action. PlonkNats is the canonical example: the
same reconciler picks between **InstallNats** (on the create / spec-
change path) and **UninstallNats** (on the deletion / finalizer-
`cleanup` path), each on its own JetStream subject, each handled by
a distinct worker with a distinct `fieldManager`. Every action
follows the same cursor pattern — reconciler publishes, worker does
ONE simple thing, worker SSA-applies its cursor condition, watch
fires, reconciler advances. See [`docs/plonk_nats.md`](docs/plonk_nats.md)
"Teardown flow" for the worked sequence diagram. Conventions:

- **One subject per action.** `plonk.tasks.<resource>.<verb>` (e.g.
  `plonk.tasks.nats.install`, `plonk.tasks.nats.uninstall`). Each
  subject corresponds to one worker module, one FlatBuffer message
  type, and one cursor condition.
- **One fieldManager per worker.** Distinct constants in
  `plonk_crds::crds` — `NATS_INSTALLER_FIELD_MANAGER`,
  `NATS_UNINSTALLER_FIELD_MANAGER`, etc. The pinned-distinct test
  (`nats_field_manager_constants_are_distinct`) prevents accidental
  aliasing that would silently collapse two workers' ownership
  into one.
- **One cursor condition per worker.** Each worker writes exactly
  one cursor condition whose `type` is unique to that action
  (install → `ResourcesApplied`, uninstall → `ResourcesReleased`).
  Uniqueness is what lets the SSA list-map merge (which uses `type`
  as the merge key) keep per-worker entries independent — two
  workers sharing a `type` would clobber each other's cursor on
  every Apply.
- **Pure decider per branch.** The reconciler's "which action
  next?" logic lives in a pure helper that takes the prior
  conditions and returns an enum of possible next steps —
  `dispatched_for_generation` for the install branch,
  `next_cleanup_action` for the teardown branch. Pure helpers are
  unit-testable without spinning up a kube client / NATS connection.
- **No worker fans out to another worker.** Workers ack and stop.
  When the next action needs to fire, the cursor condition the
  worker just wrote re-fires the reconciler's watch, and the
  reconciler decides + publishes the next task. This keeps NATS
  redelivery semantics consistent — every action is delivered to
  exactly one worker, never chained mid-flight.

When adding a third action (e.g. backup, certificate rotation,
seed-data load), copy the install/uninstall shape: new subject,
new schema, new worker file, new fieldManager constant, new cursor
condition, new pure decider branch. The reconciler stays the
single source of "what's next given the CR's current state."

**Multi-writer SSA on conditions.** Plonk has **two coexisting
patterns** for writing `.status.conditions[]`. Which one a CR uses
depends on when it was introduced:

- **Aggregator path** (PlonkBox, PlonkNamespace): workers publish
  state-change events to a NATS JetStream stream (`PLONK_STATUS`);
  one leader-gated `plonk-status-aggregator` worker holds the truth
  and is the sole writer of the conditions array. Documented in
  [`docs/architecture.md`](docs/architecture.md) under
  "Status Writes and Field-Manager Hygiene". New work on these CRs
  must keep using this path — direct condition writes from any
  fieldManager other than `plonk-status-aggregator` would un-claim
  the aggregator's entries on every reconcile.
- **Direct multi-writer SSA** (PlonkNats and every conditions-based
  CR introduced afterwards): each writer holds a distinct
  `fieldManager` and Applies its own cursor condition directly to
  `.status.conditions[]`. The SSA list-map merge keys on `type`,
  so per-worker entries stay independent without an aggregator
  collapse. This section's rules below apply to **this** pattern.

The decision rule for a new CR: if the new CR introduces new
condition types that don't exist on PlonkBox / PlonkNamespace and
the reconciler/worker split here applies, use the direct multi-
writer SSA pattern. Adding a new condition type to PlonkBox or
PlonkNamespace stays on the aggregator path.

Rules for the direct multi-writer pattern — each writer must:

- Use a distinct `fieldManager` (e.g. `plonk-nats-reconciler` vs
  `plonk-nats-installer`). The fieldManager is the field-ownership
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
PlonkStorage, PlonkNats, PlonkGateway, PlonkRoute, PlonkDomain)
already ships conditions-only. New CRs ship in the target shape
directly.

**Reason vocabulary as pinned constants.** Every `Reason` string a
condition writer can produce is a `pub const` in `plonk_crds::crds`
(e.g. `NATS_REASON_TIER_NOT_FOUND`). Producer (reconciler / worker)
and consumer (CLI's install wait, dashboards, alerts) reference the
constants by name — never inline literals — and a single test pins
the constant list as stable camel-case so drift is caught at compile
time.

**Bootstrap chicken-and-egg.** When a CR's reconciler depends on a
resource the CR itself materialises (NATS is the canonical case:
operator's startup hard-fails on NATS unreachable, so the operator
can't be the only path to bringing NATS up), the CLI bootstraps the
shape with `Create` (POST), producing the **byte-identical
resources** the operator's worker would render via SSA-apply. The
operator's first reconcile is then a no-op SSA — every field the
operator writes already matches what etcd holds, so `.force()`
transfers ownership without diff and the kube-controller computes
the same pod-template hash, leaving the running pods undisturbed.
The hard property is shape parity, not fieldManager parity:
diverging on a label value, a wrapper-script byte, a port literal,
or any other pod-template input changes the hash and triggers a
rolling restart of the operator's own broker during its own
startup. Thread every input through both sides (StorageClass,
mount path, labels, wrapper script, env vars).

