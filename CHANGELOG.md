# Changelog

All notable changes to this project will be documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] — 2026-05-23

Initial release. A Rust port of [x-motemen/ghq](https://github.com/x-motemen/ghq) 1.8.0 within the git-only subset.

### Added

- `scap get` (alias `clone`): clone or update a repository, with `-u/--update`, `-p`, `--shallow`, `--branch`, `--bare`, `--partial`, `--silent`, `--look`, `--parallel`, `--no-recursive` flags. Atomic clone via temp-dir + rename; per-target lock with exit 75 on concurrent clone.
- `scap list`: walk roots, filter by query (substring or `--exact`), `--full-path`, `--unique`, `--bare`.
- `scap rm`: interactive removal with `--dry-run` and `--bare`. Strict `y` confirmation matches ghq exactly.
- `scap root`: print repository root(s), with `--all`.
- `scap create`: initialize a new repository at the computed path, with `--bare` and `--vcs`.
- Multi-root resolution: `$GHQ_ROOT` (path-list) → reversed multi `ghq.root` → `~/ghq`. `--all` and per-URL `ghq.<url>.root` urlmatch override.
- Symlink-resolved root paths (matches ghq `local_repository.go:399-405`).
- Configurable via `git config` keys: `ghq.root`, `ghq.user`, `ghq.completeUser`, `ghq.<url>.root`.
- 76 tests across lib unit and integration suites, all running real `git config` and real local git fixtures (no mocks per project convention).

### Intentional divergences from ghq 1.8.0

- Git-only VCS (svn/hg/darcs/fossil/bzr rejected with clear message).
- Atomic clone via temp-dir + rename (Scenario A).
- Per-target lock + exit 75 on concurrent clone (Scenario B).
- `SCAP_LOOK` env (replaces `GHQ_LOOK`; no fallback).

[0.1.0]: https://github.com/zchee/scap/releases/tag/v0.1.0
