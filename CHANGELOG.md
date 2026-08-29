# Changelog

All notable changes to this project will be documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `SCAP_CONFIG_BACKEND=git` forces the whole configuration through one `git config --list -z --show-origin` subprocess, with `git` as the parser of record (ADR-8, ADR-13).

### Changed

- The gitconfig is now parsed in process with `gix-config` from an explicit source list, so `scap root`, `scap list`, `scap rm` and `scap create` spawn no `git config` at all for a configuration without url-scoped `[scap "<url>"]` sections. The system file is chosen by probing `/etc/gitconfig`, `/usr/local/etc/gitconfig`, `/opt/homebrew/etc/gitconfig` and `/opt/local/etc/gitconfig` unless `GIT_CONFIG_SYSTEM` names one. `GIT_CONFIG_COUNT`, `GIT_CONFIG_PARAMETERS`, an `includeIf` with an `onbranch:` or `hasconfig:` condition, and a probe that matches more than one file each route the whole snapshot through the `git config --list` backend instead; if no `git` is reachable at that point scap exits 1 naming the trigger rather than silently falling back (ADR-8, ADR-13).
- Repository-level configuration is read from git's *common* directory, so a linked worktree contributes `<main>/.git/config` rather than its own private `config` file, and `$GIT_DIR/config.worktree` is read only when `extensions.worktreeConfig` is enabled.
- An unparsable `scap.completeUser` or `scap.listCache` boolean now reads as false in both backends rather than diverging between them; git accepts only `yes`/`on`/`true`, `no`/`off`/`false`, an integer or the empty value, and exits fatally on anything else (ADR-13). A `scap.root` value scap cannot interpolate (`%(prefix)/`, which needs git's own installation directory) keeps its raw spelling, and a `GIT_CEILING_DIRECTORIES` entry written through a symlink is not honoured because discovery matches the resolved ancestor chain (ADR-13).
- `root_for_url` reproduces git's plain-key fallback in process: with no url-scoped section visible it returns the last `scap.root` value exactly as `git config --path --get-urlmatch` prints it — `--path`-interpolated but not symlink-resolved — instead of routing through the canonicalising root resolution.
- `is_codecommit_input` now uses ghq's pattern (`[^]]+` user class): `codecommit://a@b@c` is accepted and `codecommit://a]b@c` rejected, changing `root_for_url` dispatch for those inputs (ADR-13).

### Removed

- Dev-dependency `serial_test`. The configuration unit tests take an explicit environment view instead of mutating the process environment, which removed the last two `unsafe` blocks in the tree.
- Dependencies `regex`, `fs2` and `dirs`. The codecommit check is a hand-written matcher, the per-target clone lock uses `std::fs::File::try_lock`/`unlock`, and the home-directory fallback uses `std::env::home_dir`. `regex` is retained as a dev-dependency for the differential test against ghq's pattern.

### Fixed

- A codecommit target resolves against the canonicalised primary root again, matching ghq's `getRoot()`. `url::from_input` normalises such a target to `codecommit://<region>/<owner>/<name>`, and the root rule tested only the raw `codecommit::<region>://<repo>` spelling, so it never fired for the form the commands actually pass and the destination was built from the unresolved `scap.root` value instead.

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
