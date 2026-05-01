# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

**IMPORTANT**: Never claim something is "tracked" or "documented" without actually doing it in the same response. If you say "tracked on #X" or "documented in Y", the issue/file MUST be updated before you finish your response. Making false claims about tracking is unacceptable.

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

**CRITICAL**: Always use the Plonk CLI for E2E testing. Never manually apply YAML files or use kubectl directly for installation/uninstallation.

**Prerequisites**: E2E testing requires a k3d cluster. Use `make e2e-cluster-create` or `make e2e-full` for the complete workflow.

### Testing Workflow

**1. Build and load images:**

E2E needs two images in the k3d cluster: the operator and `rocket`, the
Plonk-conformant test fixture used as the `PlonkBox` image (it serves
`/healthz`, `/readyz`, `/metrics` on 8080 — `nginx` does not, and
`PlonkBox` probes will hang on readiness if you point them at it).

```bash
# Build + import both images (operator + rocket)
make e2e-load-images

# Or one at a time
make e2e-load-image       # operator
make e2e-load-rocket      # rocket

# For a clean rebuild (skips Docker layer cache):
make e2e-load-images DOCKER_BUILD_FLAGS=--no-cache
```

See `plonk/apps/rocket/README.md` for rocket's env-var knobs
(`ROCKET_READY_DELAY_SECS`, `ROCKET_METRICS`) — both are useful when
writing transition-style e2e tests.

**2. Uninstall existing deployment:**
```bash
cargo run --release -p plonk_cli -- uninstall --yes --namespace plonk
```

**3. Install using CLI:**
```bash
cargo run --release -p plonk_cli -- install --yes --namespace plonk --operator-image localhost:5000/plonk-operator:test
```

**4. Verify deployment:**
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
     # arbitrary images will hang on readiness.
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

