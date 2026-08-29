# Changelog

All notable changes to this project will be documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- `is_codecommit_input` now uses ghq's pattern (`[^]]+` user class): `codecommit://a@b@c` is accepted and `codecommit://a]b@c` rejected, changing `root_for_url` dispatch for those inputs (ADR-13).

### Removed

- Dependencies `regex`, `fs2` and `dirs`. The codecommit check is a hand-written matcher, the per-target clone lock uses `std::fs::File::try_lock`/`unlock`, and the home-directory fallback uses `std::env::home_dir`. `regex` is retained as a dev-dependency for the differential test against ghq's pattern.

## [0.1.0] — 2026-05-23

Initial release. A Rust port of [x-motemen/ghq](https://github.com/x-motemen/ghq) 1.8.0 within the git-only subset.

### Added

- `scap get` (alias `clone`): clone or update a repository, with `-u/--update`, `-p`, `--shallow`, `--branch`, `--bare`, `--partial`, `--silent`, `--look`, `--parallel`, `--no-recursive` flags. Atomic clone via temp-dir + rename; per-target lock with exit 75 on concurrent clone.
- `scap list`: walk roots, filter by query (substring or `--exact`), `--full-path`, `--unique`, `--bare`.
- `scap rm`: interactive removal with `--dry-run` and `--bare`. Strict `y` confirmation matches ghq exactly.
- `scap root`: print repository root(s), with `--all`.
- `scap create`: initialize a new repository at the computed path, with `--bare` and `--vcs`.
- Multi-root resolution: `$SCAP_ROOT` (path-list) → reversed multi `scap.root` → `~/scap`. `--all` and per-URL `scap.<url>.root` urlmatch override.
- Symlink-resolved root paths (matches ghq `local_repository.go:399-405`).
- Configurable via `git config` keys: `scap.root`, `scap.user`, `scap.completeUser`, `scap.<url>.root`.
- 76 tests across lib unit and integration suites, all running real `git config` and real local git fixtures (no mocks per project convention).

### Intentional divergences from ghq 1.8.0

- Git-only VCS (svn/hg/darcs/fossil/bzr rejected with clear message).
- Atomic clone via temp-dir + rename (Scenario A).
- Per-target lock + exit 75 on concurrent clone (Scenario B).
- `SCAP_LOOK` env (replaces `GHQ_LOOK`; no fallback).

[Unreleased]: https://github.com/zchee/scap/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/zchee/scap/releases/tag/v0.1.0
