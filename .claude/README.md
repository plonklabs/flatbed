# Claude Code adapter

Claude Code discovers the canonical repository policy through the root
`CLAUDE.md`. Repository safety, testing, Git, pull request, review-bot, and
architecture rules stay in `AGENTS.md`.

Shared workflow skills live in `.agents/skills/` (one directory per skill,
`SKILL.md` with Agent Skills frontmatter). The `skills/` directory here
contains tracked symlinks — one per skill, pointing to the canonical
directory — so Claude Code's slash-command discovery reads the same bodies
every other harness does.

This directory contains only Claude-native mechanics:

- `skills/` provides tracked symlinks for slash-command discovery; authored
  workflows live in `.agents/skills/`.
- `agents/` defines the FleetView-selectable persistent worker sessions
  (`worker-flatbed1..3`), including native frontmatter and Claude model pins.
- `settings.json` is checked in and team-wide: the auto-compact keys and
  three hooks whose scripts live in `hooks/`. The `PreCompact` hook injects
  the orchestrator skill's Session-economy contract into the compaction
  summarizer. The `SessionStart` hook (matching `startup`, `compact`)
  injects `fleet resume --hook` output — the ledger-derived seat frontiers
  and open watches — so a fresh or compacted session reads its operational
  position from the store, not a drifted summary; it is silent when there is
  no open work, and scopes to the worktree's own seat. The `PostToolUse`
  hook (matching `Monitor`) registers an armed Claude Monitor as a
  runtime-neutral watch descriptor via `fleet watch register --from-json`,
  so the ledger records what should be re-armed after a restart.
- `settings.local.json`, gitignored, contains optional local Claude tool
  permissions layered on top.

## Native mechanics reference

The canonical skill bodies use harness-neutral contracts. Claude Code
implements them as follows:

| Canonical contract | Claude implementation |
|---|---|
| Agent picker / fleet dashboard | FleetView (the Claude Code agent selector) |
| Live agent enumeration | `ListAgents` tool |
| Worker messaging | `SendMessage <agent-name> <message>` |
| Bounded background task tracking | `run_in_background: true` on Bash + `TaskOutput` tool |
| Fresh worker agent dispatch | `subagent_type: worker-flatbedN` (from `agents/`) |
| Bounded poller / check monitor | Monitor tool with a ≤15-minute deadline |
