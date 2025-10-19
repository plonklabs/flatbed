# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Plonk** is a Kubernetes infrastructure management platform written in Rust that provides unified secret management and service mesh control within Kubernetes clusters. It is part of the larger Winkoz Group monorepo.

The project consists of:
- **plonk_cli**: Interactive TUI-based CLI for infrastructure initialization and management
- **plonk_agent**: Kubernetes-resident agent for monitoring and telemetry
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
8. **Address PR comments**: Respond to and fix issues from code review
9. **Keep PR updated**: Continue updating description as you make fixes
10. **Finalize**: Remove `/todo.txt`, merge PR

**todo.txt format**: Use it as your working journal. Track tasks, decisions, blockers, test results, review comments, and next steps. Update frequently.

## Build and Development Commands

```bash
# Build and run
make build-plonk-cli  # or: cargo run -p plonk_cli
cargo build -p plonk_cli --release
cargo build -p plonk_agent --release
cargo build --workspace

# Test and quality
cargo test --workspace
cargo test -p plonk_cli
cargo clippy --workspace --all-targets --all-features
cargo fmt --all
```

## Key Architecture Notes

