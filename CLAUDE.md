# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

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
6. **Update PR description**: Keep PR description current with all changes
7. **Address PR comments**: See "Addressing PR Review Comments" section below
8. **Keep PR updated**: Continue updating description as you make fixes
9. **Finalize**: Merge PR

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

**1. Build and load operator image:**
```bash
# Build operator and import into k3d cluster
make e2e-load-image

# For a clean rebuild (skips Docker layer cache):
make e2e-load-image DOCKER_BUILD_FLAGS=--no-cache
```

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
     image: nginx:latest
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

