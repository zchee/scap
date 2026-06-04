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

## Toolchain

This project targets **stable Rust**. `rust-toolchain.toml` pins the channel to `stable` with `rustfmt`, `clippy`, and `rust-src` components, so plain `cargo build` / `cargo test` / `cargo fmt` / `cargo clippy` use the pinned stable toolchain automatically — do not invoke `cargo +nightly` or pin a different toolchain. If a nightly-only feature is genuinely needed, open an ADR first (see `.omc/plans/`).

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
