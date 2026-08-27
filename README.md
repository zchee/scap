# scap

A Rust port of [x-motemen/ghq](https://github.com/x-motemen/ghq).

scap manages local clones of remote git repositories under a structured root path (`<root>/<host>/<owner>/<repo>`). It mirrors ghq 1.8.0's user-facing surface — subcommands, flags, config keys, and exit semantics — within the git-only subset (v1).

> Like [General Headquarters][ghq], but for the [Supreme Commander for the Allied Powers][scap-wiki]. (jargon, joke :P)

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
