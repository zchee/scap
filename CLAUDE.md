# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

scap is a remote repository management CLI in Rust, inspired by [x-motemen/ghq](https://github.com/x-motemen/ghq). It manages local clones of remote git repositories under a structured root path.

## Design parity with ghq

Mirror ghq's user-facing surface unless there's a concrete reason to diverge. Before adding or renaming a subcommand or config key, check what ghq does (`ghq help <cmd>`, ghq's `gitconfig` keys under `ghq.*`) and match it.

- Subcommands to mirror: `get` (alias `clone`), `list`, `rm`, `root`, `create`.
- Root path resolution should follow ghq's precedence: `$SCAP_ROOT` env, then `ghq.root` git config, then `~/ghq`. scap's equivalents must be documented in code comments when they intentionally differ.
- Path layout: `<root>/<host>/<user>/<repo>` (e.g. `~/ghq/github.com/zchee/scap`).

If you're about to introduce a command, flag, or config key that doesn't exist in ghq, surface that divergence in the PR description rather than silently inventing surface area.

Divergences introduced by the optimisation plan are registered in one place: ADR-13 in `docs/plans/2026-08-28-theoretical-limit-optimization.md`, mirrored into the README "Compatibility with ghq" table and the CHANGELOG. The four that shipped in 0.1.0 — the git-only subset, the atomic clone, the per-target lock and `SCAP_LOOK` — are listed in that release's own CHANGELOG section instead. Add the row when the surface ships.

## Toolchain

This project targets **nightly Rust**. `rust-toolchain.toml` pins the channel to `nightly` with `rustfmt` and `clippy` components, so plain `cargo build` / `cargo test` / `cargo fmt` / `cargo clippy` use the pinned nightly toolchain automatically — do not pin a different toolchain. `rustfmt.toml` depends on nightly-only options (`group_imports`, `imports_granularity`, `normalize_comments`, `format_code_in_doc_comments`, `error_on_line_overflow`, `error_on_unformatted`), so formatting is not reproducible on stable. Nightly formatting behaviour can shift between dates; if `cargo fmt --check` starts failing without a source change, suspect a toolchain bump before the code.

## CLI

Use the `clap` derive API (`#[derive(Parser)]`, `#[derive(Subcommand)]`). Do not reach for `structopt` (deprecated/merged into clap) or hand-rolled argument parsing.

## Async

v1 is **synchronous**. The standard library does not ship an async executor, so an "async without a third-party runtime" instruction is unimplementable. Network-bound work uses blocking subprocess and filesystem calls; `get --parallel` uses `std::thread::scope` with a fixed pool of 6 workers (matching ghq's `cmd_get.go` semaphore size).

If a future feature genuinely needs concurrent network I/O (e.g. go-import meta-tag discovery, HTTP repository listing), open a new ADR in `.omc/plans/` before introducing a runtime. Likely candidates are `ureq` (blocking HTTP) or `pollster` + a small async HTTP crate — neither has been adopted in v1.

## Commits

- **Always GPG-sign**: `git commit --gpg-sign` (or set `commit.gpgsign=true`).
- **Conventional Commits**: `<type>(<scope>): <subject>`. Types: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `build`, `ci`, `perf`. Scope is the subcommand or module (e.g. `feat(get): add --shallow flag`, `fix(list): handle empty root`).

## Style and quality

Follow `~/.claude/instructions/Rust.md` (loaded via global CLAUDE.md). Key points reiterated for this repo:

- 4-space indent, 100-char line limit (rustfmt defaults).
- Use `tracing` for logging — never `println!` for error or status output. CLI human-facing output goes through `eprintln!`/`println!` as appropriate, but diagnostic logs use `tracing`.
- `cargo fmt` and `cargo clippy --all-targets --all-features -- -D warnings` must pass before commit.

## Testing

- Every public function gets a test.
- Integration tests exercising subcommands live in `tests/`.
- No mocking of git or the filesystem: use `tempfile` for scratch dirs and real local git repos as fixtures.

<!-- br-agent-instructions-v1 -->

---

## Beads Workflow Integration

This project uses [beads_rust](https://github.com/Dicklesworthstone/beads_rust) (`br`/`bd`) for issue tracking. Issues are stored in `.beads/` and tracked in git.

### Essential Commands

```bash
# View ready issues (open, unblocked, not deferred)
br ready              # or: bd ready

# List and search
br list --status=open # All open issues
br show <id>          # Full issue details with dependencies
br search "keyword"   # Full-text search

# Create and update
br create --title="..." --description="..." --type=task --priority=2
br update <id> --status=in_progress
br close <id> --reason="Completed"
br close <id1> <id2>  # Close multiple issues at once

# Sync with git
br sync --flush-only  # Export DB to JSONL
br sync --status      # Check sync status
```

### Workflow Pattern

1. **Start**: Run `br ready` to find actionable work
2. **Claim**: Use `br update <id> --status=in_progress`
3. **Work**: Implement the task
4. **Complete**: Use `br close <id>`
5. **Sync**: Always run `br sync --flush-only` at session end

### Key Concepts

- **Dependencies**: Issues can block other issues. `br ready` shows only open, unblocked work.
- **Priority**: P0=critical, P1=high, P2=medium, P3=low, P4=backlog (use numbers 0-4, not words)
- **Types**: task, bug, feature, epic, chore, docs, question
- **Blocking**: `br dep add <issue> <depends-on>` to add dependencies

### Session Protocol

**Before ending any session, run this checklist:**

```bash
git status              # Check what changed
git add <files>         # Stage code changes
br sync --flush-only    # Export beads changes to JSONL
git commit -m "..."     # Commit everything
git push                # Push to remote
```

### Best Practices

- Check `br ready` at session start to find available work
- Update status as you work (in_progress → closed)
- Create new issues with `br create` when you discover tasks
- Use descriptive titles and set appropriate priority/type
- Always sync before ending session

<!-- end-br-agent-instructions -->
