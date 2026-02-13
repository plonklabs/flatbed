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
1. **Start from main**: `git checkout main && git pull && git rebase origin/main`
2. **Create feature branch**: `git checkout -b feature/your-feature-name`
3. **Create todo.txt**: Create `/todo.txt` at repo root as your PR journal
4. **Develop**: Implement changes, write tests, run lints
5. **Test automation**: Create automated tests for your changes
6. **Clean up**: Format code, remove debug statements, update comments
7. **Update PR description**: Keep PR description current with all changes
8. **Address PR comments**: See "Addressing PR Review Comments" section below
9. **Keep PR updated**: Continue updating description as you make fixes
10. **Finalize**: Remove `/todo.txt`, merge PR

**todo.txt format**: Use it as your working journal. Track tasks, decisions, blockers, test results, review comments, and next steps. Update frequently.

## Addressing PR Review Comments

When asked to address PR review comments, follow this workflow:

1. **Fetch comments**: Use `gh api repos/{owner}/{repo}/pulls/{pr_number}/comments` to get all review comments

2. **For each comment**, decide whether to:
   - **Fix it**: Make the change in a separate commit
   - **Decline it**: Explain why (design decision, out of scope, etc.)

3. **Create separate commits**: Each fix should be its own commit with a clear message referencing the issue

4. **Reply directly to review comments**: Use the GitHub API to reply in the comment thread:
   ```bash
   gh api repos/{owner}/{repo}/pulls/{pr_number}/comments/{comment_id}/replies -X POST -f body='Your reply'
   ```
   - If fixed: Include the commit SHA (e.g., "Fixed in commit abc123")
   - If declined: Explain the reasoning clearly and respectfully

5. **Example workflow**:
   ```bash
   # Get PR review comments
   gh api repos/winkoz/plonk/pulls/24/comments

   # Make fix and commit
   git add <files>
   git commit -m "Fix: address review comment about X"
   git push

   # Reply directly to the comment thread
   gh api repos/winkoz/plonk/pulls/24/comments/123456789/replies -X POST -f body='Fixed in commit abc123'
   ```

6. **General PR comments** (not in a thread): Use `gh pr comment {pr_number} --body "message"` for standalone comments not tied to specific code lines.

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

### Testing Workflow

**1. Build operator image:**
```bash
# Build without cache to ensure latest code
docker build --no-cache -t plonk-operator:test -f plonk/apps/operator/Dockerfile .

# Load into Minikube (replace 'plonk-one' with your profile)
minikube image load plonk-operator:test --profile plonk-one
```

**2. Uninstall existing deployment:**
```bash
cargo run --release -p plonk_cli -- uninstall --yes --namespace plonk
```

**3. Install using CLI:**
```bash
cargo run --release -p plonk_cli -- install --yes --namespace plonk --operator-image plonk-operator:test
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
   apiVersion: plonk.com/v1
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
- Minikube caches images. Use `--no-cache` when building and verify with a new tag
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

## Specs

Design specifications live in `specs/`. Each spec has its own folder with a `spec.md` and optional step files (`step1.md`, `step2.md`, etc.) for execution planning. See `specs/README.md` for details.

## Key Architecture Notes

