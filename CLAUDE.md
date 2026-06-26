# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working
with code in this repository.

**IMPORTANT**: Never claim something is "tracked" or "documented" without
actually doing it in the same response. If you say "tracked on #X" or
"documented in Y", the issue/file MUST be updated before you finish your
response. Making false claims about tracking is unacceptable.

## Minimalism + pragmatism

> *"Lo que no sirve que no estorbe"* — what doesn't serve a purpose
> shouldn't be in the way.

Keep the codebase (code, docs, scripts, configs, tests, CI surface) as
**minimal as possible while being as pragmatic as possible**. The two
halves are equally weighted: aggressive deletion of dead weight, but not
at the cost of breaking real workflows, losing genuine context, or
shipping cute-but-fragile abstractions.

- **Code.** Dead modules, unused helpers, compatibility shims for
  migrations that already completed, "future-proofing" abstractions for
  hypothetical needs — delete. Three similar lines beats a premature
  trait.
- **Docs.** Historical / superseded docs rot fastest of all (every line
  is a claim about code that has since moved). Confusion-on-read →
  delete.
- **Comments.** Default to not writing them. See the Comments section.
- **Tests.** Skip the test for the speculative case nobody will ever
  hit; keep the test for the regression that actually happened.
- **CI / scripts.** Targets that nobody runs, env-var knobs with no
  remaining caller — delete.

## Project Overview

**flatbed** is a Rust HTTP framework for services that sit inside
Kubernetes pods behind an Envoy sidecar. Three crates:

- **`flatbed`** — the framework runtime: Hyper-backed server, route
  registry, request/response types, error handling, optional
  `telemetry` / `openapi` / `nats` / `k8s` feature gates.
- **`flatbed_macros`** — procedural macros: `#[route]`, `#[worker]`,
  `#[flatbed::main]`.
- **`flatbed_build`** — build-time FlatBuffer codegen and the
  standalone `flatbed` CLI tool that drives codegen from `.fbs`
  schemas.

The framework was extracted from `plonklabs/plonk` at v0.0.1. Its
design biases: zero TLS in the framework (the sidecar handles it),
zero magic dependency injection (handlers take typed `Request<T, C>`),
build-time route validation via `inventory`.

## Build and Development Commands

```bash
# Build all crates
cargo build --workspace

# Build the codegen binary
cargo build -p flatbed_build --bin flatbed --release

# Test
cargo test --workspace
cargo test --workspace --all-features

# Lint
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings

# Verify committed FlatBuffer codegen matches the `.fbs` schemas
bash scripts/check-generated.sh
```

The `flatc` binary must be installed and on `PATH` at the version
pinned in `.flatc-version`. Version drift causes byte-level diffs in
the generated code and makes `check-generated.sh` report stale on every
PR. Bump `.flatc-version` and the CI runner's `flatc` install in
lockstep.

## PR Workflow

1. Branch from latest `main`: `git fetch origin main && git switch -c
   feature/your-feature origin/main`.
2. Implement.
3. Run the gate suite: `cargo fmt --all && cargo clippy --workspace
   --all-targets --all-features -- -D warnings && cargo test
   --workspace`.
4. Open a **draft** PR via `gh pr create --draft`. The bot reviewer
   runs on `ready_for_review`, so the draft state is your local
   self-review window.
5. Flip to ready when you're satisfied.
6. Address any inline bot threads, push fix commits, and reply on each
   thread with the commit SHA.

## Coding Style Guidelines

- **Favor let-else over nesting**: Use `let-else` patterns with early
  returns instead of deeply nested `if-let` blocks.
- **Keep functions flat**: maximum 1-2 levels of indentation.
- **Early returns** on error conditions and guards.
- **Use `?`** for error propagation, not explicit `match`.
- **No wildcard imports**: never `use foo::*`; import specific items.
- **Document public APIs**: all public functions, types, and modules
  need doc comments.
- **`cargo clippy --workspace --all-targets --all-features -- -D
  warnings`** before committing.

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

1. **Cannot be inferred from the surrounding code or types.**
2. **Will be useful to a future reader,** not just the author at write
   time.
3. **Will remain accurate through reasonable refactors.**

#### When a comment earns its place

- **Constraints from outside the code.** A platform behaviour, library
  quirk, or wire-format requirement the types can't express.
- **Invariants the caller is responsible for.** Contracts the type
  system can't enforce, made explicit so a future caller doesn't break
  the function by accident.
- **Surprising library or language behaviour.** A standard-library or
  external API that doesn't behave the obvious way; the comment names
  the foot-gun.
- **Workarounds for a bug, named by its property.** Not by issue number
  (which rots), but by the bounded condition under which the workaround
  is load-bearing.

In every case the comment describes a *property of the world the code
runs in*, not a property of the change that introduced it. That's what
makes it durable.

#### Patterns that rot the fastest

- **Restating what the code already says.** `// increment the counter`
  next to `i += 1`. The reader spends attention parsing both with
  nothing gained.
- **Forward-looking or temporal phrasing.** "Future X will…", "for when
  we add Y", "the new path", "the pre-cutover overlap". Wrong the
  moment X is renamed, Y never ships, "new" becomes the default, or
  the cutover completes. State the property without dating it.
- **References to other code, docs, or issues by path or name.** The
  referenced thing keeps compiling fine when it's renamed or moved;
  the comment silently points at nothing. State the property the
  reference would have explained, inline.
- **Reasoning about the patch instead of the code.** Reasoning about a
  *change* belongs in the PR description and commit message. Source
  comments have to keep being accurate as the code keeps moving — a
  poor home for any thought scoped to "the state of the world when I
  wrote this".
- **Author-mental-state asides.** "Tricky", "TODO: probably refactor",
  "not sure why this works". Either fix what the comment is hedging
  about, or replace the hedge with a specific statement about the
  invariant.

#### The self-test

Before keeping or writing a comment, walk it through three questions:

1. **Removal test.** If I deleted this comment, would a reader with the
   surrounding code and a working knowledge of the codebase actually
   be confused?
2. **Refactor test.** Could a sensible nearby restructure leave the
   comment quietly wrong without anyone noticing?
3. **Scope test.** Is the comment about the *code as it stands*, or
   about the *change that produced it*?

Code-as-it-stands belongs in the source; change-reasoning belongs in
the PR description and commit message.

## Worker and Route Discovery

The `#[worker]` and `#[route]` macros use the `inventory` crate for
**compile-time registration**:

- **No re-exports needed**: simply declaring a module (`mod workers;`)
  is sufficient. The macros register items automatically via
  `inventory::submit!`.
- **Don't suppress unused-import warnings**: if the compiler says a
  re-export is unused, it's genuinely unused. The inventory system
  discovers items regardless of module visibility.
- Workers are spawned automatically by `Flatbed::run()`.

```rust
// workers/mod.rs — correct pattern
mod deploy;  // Just declare, no `pub use` needed

// workers/deploy.rs
#[worker(name = "my-worker", description = "Does work")]
pub async fn my_worker(ctx: Arc<AppContext>) -> Result<(), FlatbedWorkerError> {
    // Worker logic
}
```

## Releases

- crates.io publishing is automated via `.github/workflows/publish.yml`,
  triggered by `flatbed-v*` tag pushes. The workflow uses the
  `CARGO_REGISTRY_TOKEN` repo secret.
- The standalone `flatbed` CLI binary is published as a GitHub release
  asset by `.github/workflows/release-bin.yml` on the same tag push.
- Version bumps live in `[workspace.package]` of the root `Cargo.toml`.
  Per Cargo semver-compat, every `0.0.x → 0.0.(x+1)` is treated as
  breaking, so pre-1.0 releases can change the public surface freely.
