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

## License

Apache-2.0. See [LICENSE](LICENSE).

[ghq]: https://en.wikipedia.org/wiki/General_Headquarters
[scap-wiki]: https://en.wikipedia.org/wiki/Supreme_Commander_for_the_Allied_Powers
