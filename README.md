# scap

A Rust port of [x-motemen/ghq](https://github.com/x-motemen/ghq).

scap manages local clones of remote git repositories under a structured root path (`<root>/<host>/<owner>/<repo>`). It mirrors ghq 1.8.0's user-facing surface — subcommands, flags, config keys, and exit semantics — within the git-only subset (v1).

> [!NOTE]
> Like [G eneral H ead Q uarters][ghq], but for the [S upreme C ommander for the A llied P owers][scap-wiki]. (jargon, joke :P)

## Install

```sh
cargo install --git https://github.com/zchee/scap
```

Requires git on PATH (scap shells out to system git for all VCS operations).

## Usage

```sh
scap get <repository>           # clone (alias: scap clone)
scap list [-p] [-e] [<query>]   # list local repositories
scap rm <repository>            # remove (with confirmation)
scap root [--all]               # show repository root(s)
scap create <user>/<project>    # create new repository
```

See `scap <cmd> --help` for full flag tables.

## Configuration

scap reads ghq's gitconfig keys directly:

| Key | Behavior |
|---|---|
| `ghq.root` (multi) | Repository roots, reversed before use; falls back to `~/ghq`. |
| `ghq.<url>.root` | Per-URL root override (urlmatch). |
| `ghq.user` | Default user for 1-segment input (`scap get myproj`). |
| `ghq.completeUser` | Whether to auto-complete the user from `ghq.user`. |

Environment: `$SCAP_ROOT` (colon-separated path list) wins over gitconfig.

## Compatibility with ghq

scap aims for byte-for-byte parity with ghq 1.8.0 within the git subset. Intentional divergences (each documented in code):

| Divergence | Rationale |
|---|---|
| Git-only VCS (rejects `--vcs svn`/`hg`/`darcs`/`fossil`/`bzr`) | v1 scope. Each non-git backend planned as additive PR with its own parity-check pass. (ADR-2) |
| Atomic clone via temp-dir + rename | Prevents half-cloned state after Ctrl-C. ghq's clone is not atomic. (Plan §4 Scenario A) |
| Per-target lock + exit 75 on concurrent clone | Prevents two scap processes from corrupting the same repo. ghq does not lock. (Plan §4 Scenario B) |
| `SCAP_LOOK` env (replaces ghq's `GHQ_LOOK`) | Clean branding; no fallback. Users with existing ghq shell hooks must update. |
| `SCAP_CONFIG_BACKEND=git` env | Forces the whole configuration through one `git config --list` subprocess. ghq always spawns `git config`; scap parses the gitconfig in process by default, and this is the escape hatch when you want git to be the parser of record. (ADR-8/ADR-13) |
| Gitconfig read in process; `git` spawned only on explicit triggers | The system file is chosen by probing `/etc/gitconfig`, `/usr/local/etc/gitconfig`, `/opt/homebrew/etc/gitconfig` and `/opt/local/etc/gitconfig` unless `GIT_CONFIG_SYSTEM` names one. `GIT_CONFIG_COUNT`, `GIT_CONFIG_PARAMETERS`, an `includeIf` with an `onbranch:`/`hasconfig:` condition, or more than one probe match route the whole snapshot to the `git config --list` backend instead of diverging silently; if no `git` is reachable then, scap exits 1 naming the trigger rather than falling back. `scap get` spawns one `git` process per target — the clone — and none for configuration. (ADR-8/ADR-13) |
| An invalid `scap.completeUser` / `scap.listCache` boolean reads as false | git accepts only `yes`/`on`/`true`, `no`/`off`/`false`, an integer, or the empty value, and exits fatally on anything else. scap cannot exit over a key it merely happens to read, so an unparsable value takes the conservative value in both backends. (ADR-8) |
| A `scap.root` value scap cannot interpolate keeps its raw spelling | `git config --path` expands `%(prefix)/` against git's own installation directory, which scap does not have. `~/` and `~user/` are expanded exactly as git expands them. (ADR-8) |
| `GIT_CEILING_DIRECTORIES` written through a symlink is not honoured | Repository discovery matches ceiling directories against the symlink-resolved ancestor chain it walks; git matches the literal spelling. Give ceilings in their physical spelling. (ADR-8, `gix-discover`) |
| `scap.listExclude` (multi-valued) and `SCAP_LIST_EXCLUDE` (colon-separated) | Directories whose root-relative path matches one of these wildmatch patterns are neither descended nor listed, which is the only way to keep `list` off a large subtree that holds no repositories — on the author's own corpus one pattern takes the walk from 17,777 directory reads to 2,042 and changes not a byte of the output. Patterns are anchored at the root (`foo` means `<root>/foo`, not `<root>/bar/foo`) and use git's own `WM_PATHNAME` semantics, so `*` stops at a `/` and `**` crosses it. A trailing `/` is folded away, so `node_modules/` and `node_modules` are the same pattern, and matching is case-sensitive against the on-disk spelling (git sets no ignore-case flag either) — which is worth knowing on a case-insensitive APFS volume, where `Node_Modules` will not match. A non-empty `SCAP_LIST_EXCLUDE` replaces the configured patterns wholesale, as `SCAP_ROOT` replaces `scap.root`; it is split on `:`, so a pattern containing a literal colon can only be written through the config key. ghq has no equivalent. (ADR-9, ADR-13) |
| `SCAP_LIST_THREADS` env (1..=64) | Worker threads `scap list` walks each root on. The default, 4, is the smallest count whose median wall time came within 1.10× of the best on the author's corpora while keeping system time within 1.5× of the single-threaded run; it was measured on one machine's core count and filesystem, and neither is universal, so the number is reachable. A value outside `1..=64`, or one that is not a number, warns and uses the default rather than failing the listing. The repository set and the printed bytes are identical at every thread count. ghq has a fixed pool and no equivalent knob. (ADR-9, ADR-13) |
| `SCAP_LIST_DETECT` env (`stat` or `open`) | How `scap list` decides a directory is a repository. `stat` — the default — asks for `<dir>/.git` and never opens the repository, which is also what ghq does; `open` reads the directory and looks for `.git` among the entries it already has. Both find the same repositories and print the same bytes, and both were measured at the default thread count on the author's corpora: `stat` was faster on all of them, including the directory-dense one `open` was expected to win, so it is the one that ships. The knob is reachable because that measurement is one machine's filesystem and one corpus shape — around twelve directories per repository — and a tree far sparser in repositories is where `open` could still win. An unrecognised value warns and uses the default. The choice does change `dirs_read` on the walk span, which counts repository directories only under `open`. ghq has no equivalent knob. (ADR-9, ADR-13) |
| A printed path never leaves the root that names it | Each repository is named by the first configured root that contains it, which is ghq's rule and now scap's. ghq applies it with a raw text comparison, so a root `one` listed before a sibling root `onetwo` claims the sibling's repositories and prints them as `../onetwo/github.com/a/x` — a path outside every configured root, which no command can act on and which ghq's own `-p` output contradicts. scap requires the match to fall on a whole path component, so each root names its own tree whatever the order. Reversing the order makes ghq agree. (ADR-9, ADR-13) |
| `scap list` exits 0 when its output is a closed pipe | `scap list \| head -1` finishes with status 0: once the reader is gone there is nothing left to print, which is not a failure of the listing. ghq dies of `SIGPIPE` and exits 141 there, and scap before this walker exited 1 — but only for a listing too large for the kernel's pipe buffer, so the status depended on how many repositories the machine held. Status 0 keeps the printed bytes as the contract and is the friendlier answer under `set -o pipefail`. (ADR-9, ADR-13) |
| `scap list` skips a root whose `stat` fails with something other than "not found", and says so | A root that does not exist is skipped silently and an unreadable root is skipped with a warning, both matching ghq; the third case has no oracle, because ghq dereferences a nil `FileInfo` there and panics (`local_repository.go:321`, confirmed against ghq 1.8.0 with a root path below a regular file). scap warns and carries on with the remaining roots. (ADR-9, ADR-13) |
| A region-less `codecommit://` ref resolves its region like ghq, with two narrower rules | ghq reads `AWS_REGION`, then `AWS_DEFAULT_REGION`, then the stdout of `aws configure get region`, and exits 1 with `You must specify a region.` if none resolve; scap now does the same. It stays narrower in two places: an explicitly empty `AWS_REGION` is treated as absent (ghq treats it as present and would resolve to an empty region), and a failed resolution always reports scap's own message rather than forwarding the `aws` CLI's stderr, so the text does not depend on the installed `aws` version. (ADR-13) |

See `.omc/plans/2026-05-23-ghq-port-rust.md` for full ADRs and design rationale.

## Development

Toolchain pinned to nightly Rust via `rust-toolchain.toml`. Project conventions in `CLAUDE.md`.

Quality gates (all must pass before commit):

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

Parity-check skill at `.agents/skills/ghq-parity-check/SKILL.md` runs before adding or renaming any CLI surface element.

## Benchmarks

Two harnesses measure different things and are not comparable to each other. The hyperfine harness under `docs/benchmarks/` is the optimization plan's gate: whole-process wall-clock runs on a quiet machine under a fixed `RUSTFLAGS` regime, used to accept or reject a change against a frozen threshold. [CodSpeed](https://codspeed.io) is the per-commit history: the divan micro-benchmarks in `benches/` run under its deterministic CPU simulation, which is stable enough for shared CI runners, and the whole-program targets in `codspeed.yaml` run under its walltime instrument, which needs dedicated hardware and is therefore opt-in. CI builds with clean flags, so a CodSpeed number and a `docs/benchmarks/` number will not agree even for the same command.

Locally:

```sh
cargo bench                        # plain divan tables, no CodSpeed involved
cargo codspeed build -m simulation # compile the same benches instrumented
scripts/bench-fixture.sh           # generate target/bench-fixture for codspeed.yaml
```

Simulation mode is Linux-only, so on macOS the local check is that the benches build and run as plain divan; the instrumented numbers come from CI. One thing has to be done by hand before CodSpeed reports anything: enable `zchee/scap` on codspeed.io; the workflow authenticates with OpenID Connect, so no `CODSPEED_TOKEN` secret is needed. Uploading a run from a developer machine additionally needs `codspeed auth login`.

## License

Apache-2.0. See [LICENSE](LICENSE).

[ghq]: https://en.wikipedia.org/wiki/General_Headquarters
[scap-wiki]: https://en.wikipedia.org/wiki/Supreme_Commander_for_the_Allied_Powers
