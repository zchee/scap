# Changelog

All notable changes to this project will be documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `benches/config.rs` now benchmarks `ConfigSnapshot::root_for_url` (ADR-8 rules (a), (b)/(e) and (c), the last also over 8 plain roots) on a snapshot built once outside the timed body; `root_for_url` is `pub` for the benchmark to reach, delegated to by the existing free `config::root_for_url` function.
- Per-commit performance tracking on [CodSpeed](https://codspeed.io), in two layers. `benches/url.rs` and `benches/config.rs` are divan micro-benchmarks over the two pure hot paths — repository-input parsing and the ADR-8 in-process configuration load — measured under CodSpeed's deterministic CPU simulation, which is stable enough for a shared CI runner. `codspeed.yaml` adds whole-program `scap root` and `scap list` runs over a generated 1,800-repository tree (`scripts/bench-fixture.sh`), measured in walltime because process startup, the configuration file reads and the parallel walk's system calls are exactly what simulation cannot see; that job needs a dedicated runner and stays opt-in behind a repository variable. Neither layer replaces the hyperfine harness in `docs/benchmarks/`, and their numbers are not comparable to it: CI builds with clean flags where those rows are measured on a quiet machine under the plan's `RUSTFLAGS` regime.
- `SCAP_CONFIG_BACKEND=git` forces the whole configuration through one `git config --list -z --show-origin` subprocess, with `git` as the parser of record (ADR-8, ADR-13).
- `scap.listExclude` (multi-valued) and `SCAP_LIST_EXCLUDE` (colon-separated) exclude directories from `scap list`. A directory whose root-relative path matches one of the patterns is neither read nor listed, so a subtree that holds no repositories costs nothing to skip: on the author's corpus one pattern takes the walk from 16,933 directory reads to 1,198 without changing a byte of the output. Patterns are anchored at the root — `foo` excludes `<root>/foo` and not `<root>/bar/foo` — and are matched with git's own wildmatch under `WM_PATHNAME`, so `*` stops at a `/` while `**` crosses it. A non-empty `SCAP_LIST_EXCLUDE` replaces the configured patterns rather than adding to them, the way `SCAP_ROOT` replaces `scap.root` (ADR-9, ADR-13).
- `SCAP_LOG=debug` emits one `scap::walk::root{path, dirs_read, excluded, repos, threads}` span per root walked, where `dirs_read` counts the directories actually opened and `excluded` the directories pruned by `scap.listExclude`. The counters are what makes an exclusion pattern's effect measurable without guessing at it.
- `SCAP_LOG=debug` emits one `scap::config::urlmatch{url, spawned, urlmatch_spawns}` span per url-scoped root lookup, where `spawned` says whether that lookup ran `git` or was answered from the per-process memo and `urlmatch_spawns` is the running number of delegations this process has run. The field is no longer declared on `scap::config::load`, which closes before any urlmatch can run and could only ever report zero there; `reason = url_sections` on that span still names the trigger.

### Changed

- The gitconfig is now parsed in process with `gix-config` from an explicit source list, so `scap root`, `scap list`, `scap rm` and `scap create` spawn no `git config` at all for a configuration without url-scoped `[scap "<url>"]` sections. The system file is chosen by probing `/etc/gitconfig`, `/usr/local/etc/gitconfig`, `/opt/homebrew/etc/gitconfig` and `/opt/local/etc/gitconfig` unless `GIT_CONFIG_SYSTEM` names one. `GIT_CONFIG_COUNT`, `GIT_CONFIG_PARAMETERS`, an `includeIf` with an `onbranch:` or `hasconfig:` condition, and a probe that matches more than one file each route the whole snapshot through the `git config --list` backend instead; if no `git` is reachable at that point scap exits 1 naming the trigger rather than silently falling back (ADR-8, ADR-13).
- Repository-level configuration is read from git's *common* directory, so a linked worktree contributes `<main>/.git/config` rather than its own private `config` file, and `$GIT_DIR/config.worktree` is read only when `extensions.worktreeConfig` is enabled.
- An unparsable `scap.completeUser` or `scap.listCache` boolean now reads as false in both backends rather than diverging between them; git accepts only `yes`/`on`/`true`, `no`/`off`/`false`, an integer or the empty value, and exits fatally on anything else (ADR-13). A `scap.root` value scap cannot interpolate (`%(prefix)/`, which needs git's own installation directory) keeps its raw spelling, and a `GIT_CEILING_DIRECTORIES` entry written through a symlink is not honoured because discovery matches the resolved ancestor chain (ADR-13).
- `root_for_url` reproduces git's plain-key fallback in process: with no url-scoped section visible it returns the last `scap.root` value exactly as `git config --path --get-urlmatch` prints it — `--path`-interpolated but not symlink-resolved — instead of routing through the canonicalising root resolution.
- `is_codecommit_input` now uses ghq's pattern (`[^]]+` user class): `codecommit://a@b@c` is accepted and `codecommit://a]b@c` rejected, changing `root_for_url` dispatch for those inputs (ADR-13).
- `scap get` reads `scap.user` and `scap.completeUser` from the process configuration snapshot once per run instead of once per target, and `--look` resolves its destination from that same snapshot rather than reopening the configuration. For a configuration without url-scoped `[scap "<url>"]` sections, `scap get` now spawns exactly one `git` process per target — the clone itself; under `SCAP_CONFIG_BACKEND=git` it spawns one additional `git config --list` for the whole run.
- The stale temporary-directory sweep tests pid liveness with `kill(pid, 0)` through the `rustix` crate's safe wrapper instead of running a `kill -0` subprocess per candidate, which removes the last non-VCS process `scap get` created. A pid that exists but belongs to another user (`EPERM`) now counts as alive and its directory is left alone; a suffix that is not a positive pid (`0`, or a negative number) counts as dead and its directory is removed, where `kill -0 0` previously addressed the caller's own process group and reported it alive.
- For a gitconfig that does hold url-scoped `[scap "<url>"]` sections, scap runs `git config --path --get-urlmatch scap.root <url>` at most once per distinct URL per process instead of once per resolution. `scap get --parallel` over repeated targets therefore pays one delegation per distinct URL — against ghq's three `git config` spawns per target — and a configuration without such sections still spawns no `git` at all for configuration. The resolved root is unchanged: git remains the authority for every url-scoped answer.

### Removed

- Dev-dependency `serial_test`. The configuration unit tests take an explicit environment view instead of mutating the process environment, which removed the last two `unsafe` blocks in the tree.
- Dependencies `regex`, `fs2` and `dirs`. The codecommit check is a hand-written matcher, the per-target clone lock uses `std::fs::File::try_lock`/`unlock`, and the home-directory fallback uses `std::env::home_dir`. `regex` is retained as a dev-dependency for the differential test against ghq's pattern.

### Fixed

- `scap list` no longer skips an unreadable directory in silence. A directory it cannot read is reported on stderr as `<path>: Permission denied` and the walk continues with exit status 0, which is byte-for-byte what ghq prints for the same tree (`local_repository.go:301-306`). Previously it was discarded, so a listing shortened by a permissions problem looked complete. Every other walk error — a dangling symlink resolved because symlinks are followed, a path component that is not a directory — is reported at debug level instead, visible under `SCAP_LOG=debug` and silent by default, because ghq prints nothing for those and a single stale symlink anywhere in a tree would otherwise put a line on every run.
- `scap list` handles a root it cannot `stat`. A root that does not exist is skipped silently, as ghq skips it; a root that exists but has no read bit set is skipped with the same `Permission denied` warning; and a root whose `stat` fails for any other reason — a parent that cannot be searched, or a path below a regular file — is skipped with a warning naming the error. ghq panics on that third case, so it is a deliberate divergence rather than a parity fix (ADR-13).
- A codecommit target resolves against the canonicalised primary root again, matching ghq's `getRoot()`. `url::from_input` normalises such a target to `codecommit://<region>/<owner>/<name>`, and the root rule tested only the raw `codecommit::<region>://<repo>` spelling, so it never fired for the form the commands actually pass and the destination was built from the unresolved `scap.root` value instead.
- A codecommit target's destination no longer gets an inserted owner segment. `url::finalize_codecommit` built `codecommit://<region>/<owner>/<name>`, defaulting `<owner>` to the literal string `codecommit` whenever the ref carried no `<profile>@`, so `scap create codecommit::<region>://<repo>` resolved to `<root>/<region>/codecommit/<repo>` where ghq resolves to `<root>/<region>/<repo>` (`local_repository.go:76-86`: `pathParts = [Hostname()] + Path.split("/")`, and `Path` is the bare repo name, so the slice is always exactly `[region, repo]`). The optional `<profile>@` is parsed the same as before but, like ghq's `User`, no longer feeds the destination at all — with or without a profile the destination is `<root>/<region>/<repo>`. Verified against the real `ghq` 1.8.0 binary for 6 spellings (region × profile × underscore/dot/hyphen repo names).
- A codecommit ref with no explicit `::<region>:` (`codecommit://<repo>`, with or without `<profile>@`) now resolves its region the way ghq does: `AWS_REGION` if non-empty, else `AWS_DEFAULT_REGION` if non-empty, else the trimmed stdout of `aws configure get region` if that exits 0 with non-empty output, else scap exits 1 with ghq's own message — `You must specify a region. You can also configure your region by running "aws configure".` (`url.go:63-97`). Previously this spelling silently resolved to a literal placeholder host, `"codecommit"`, rather than a real region or a failure. The `aws` subprocess only runs for a region-absent codecommit ref when both variables are absent or empty; every other command, and every region-explicit codecommit ref, spawns nothing new. Two deliberate simplifications remain (ADR-13): an explicitly empty `AWS_REGION` is treated as absent where ghq treats it as present, and a failed resolution always reports scap's own message rather than forwarding the `aws` CLI's stderr.

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
