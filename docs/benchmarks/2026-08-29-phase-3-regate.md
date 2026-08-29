# Phase-3 re-gate on the unchanged walker (2026-08-29)

Ledger #21e. The Phase-3 gate was ruled **PENDING** in `ce0b926`, not passed:
AC-3's a+b wall legs sat inside the run-to-run spread, and the thread rule
returned no admissible `N` on one sweep. This document re-gates the same
program honestly — one measuring lane on the host, an ensemble rule for the
boundary legs, and a verdict per leg that is either **MET** or
**AT-THE-BOUND**, never "passed on rows a re-run can flip".

**Result: AC-3a, AC-3c and AC-3d are AT-THE-BOUND. The thread rule again
selects no admissible `N`. Every other criterion is MET.** No plan ruling is
written here; the ruling is the plan owner's.

The finding that decides how to read all of it is in
[Same-window reference](#same-window-reference-the-drift-question-answered):
a stable ABAB bracket shows the machine is about 6 % slower today than when
the bounds were frozen, **and** that this does not rescue the wall criterion,
because the ratio form against a live reference fails too.

## Environment

| | |
| --- | --- |
| Host | Apple M3 Max, 12 performance + 4 efficiency cores |
| OS | macOS 27.0 (26A5421a) |
| Toolchain | rustc 1.100.0-nightly (e457a7b0d 2026-08-27) |
| hyperfine | 1.20.0 |
| Oracle | ghq 1.8.0 |
| `RUSTFLAGS` | equal to `FROZEN_RUSTFLAGS`; `rustflags_match: true` in all seven groups |
| Forced | `forced: false` in all seven groups |
| Window | 2026-08-29, 15:22Z – 16:13Z |

## The program measured

No binary was rebuilt for this re-gate. `git diff 0b352d2..HEAD -- src/
Cargo.toml Cargo.lock` is **empty** — only documentation has changed since the
Phase-3 freeze commit — and the in-place binary already carries the Phase-3
end fingerprint, so a matching hash is stronger evidence than a fresh build
and costs the host nothing.

| | Bytes | `__TEXT` | sha256 |
| --- | --- | --- | --- |
| Phase-3 end (`#21b`) | 1,761,888 | 1,327,104 | `86dd283519696d44…` |
| Measured here | 1,761,888 | 1,327,104 | `86dd283519696d44…` |

Verified before the first timed row, after the last, and again after the Rust
gate suite. The dev-profile cargo configuration targets tmpfs, so the gate
suite cannot touch `target/release`.

A rule earned during setup and worth keeping: **in a shared worktree, confirm
`git status --short -- src/` is clean before any release build.** When this
lane started, the tree still carried another lane's uncommitted
`src/walk/pool.rs` edits — a patch its own lane had already ruled would not
land. Building then would have compiled a rejected change into a binary
called "HEAD".

## Harness hardening

`6b74995` `build(bench): refuse to run beside a foreign hyperfine`.

Two lanes measuring at once invalidates both, and the quiet gate could not
catch it: one benchmark process does not move a 16-core machine's idle figure
past an 85 % floor, so rows were admitted beside a competitor and identified
as contaminated only afterwards. The new clause tests **ownership rather than
activity** — the harness's own hyperfine always exports under `$OUT`, so any
hyperfine whose `--export-json` lies outside this run's `$OUT` is foreign,
including one exporting elsewhere under this same repository. A hyperfine with
no `--export-json` cannot prove it is ours and counts as foreign too.

Scanned with `pgrep -x`, not `pgrep -f hyperfine`: the latter also matches the
caller's own shell command line, which was reproduced during development —
the gate flagged itself. File mtimes are deliberately not consulted; a history
restore rewrote older run directories onto disk mid-window once already.
`SCAP_BENCH_FORCE` does not bypass it. A detection appends
`OUT/CONTAMINATED` naming the process and exits 3, labelling the partial
directory rather than deleting the evidence.

Zero foreign hyperfine were seen: **49 scans across the seven groups, all
clean.**

## Window discipline

Seven groups admitted. **24 gate refusals** and **11 groups discarded**, every
discard for a *closing*-gate failure.

The harness gates the **start** of a group and only **records** the close. The
first window-2 attempt began with the busiest process at 10.4 % and ended at
31.8 %: quiet when it was admitted, contended while it measured. That is the
shape of the group `#21b` had to withdraw. So the closing sample is held to
the same thresholds — idle ≥ 85 %, busiest ≤ 15 % — and a group failing it is
set aside and re-taken.

That check lives in the lane's operator wrapper, not in the harness.
**Enforcing it inside `scripts/bench-quiet.sh` is a follow-up `build(bench)`
item**, recorded in the ledger rather than smuggled into this commit.

Discarded directories were moved out of the tree, not deleted. Their `list_ab`
medians read 139.6–142.8, consistent with the admitted windows.

| Group | Run directory | Refusals | Idle start→end | Busiest start→end |
| --- | --- | --- | --- | --- |
| W1 | `20260829T152228Z-p3regate-w1` | 0 | 89.10 → 90.70 | 9.9 → 10.3 |
| W2 | `20260829T153917Z-p3regate-w2` | 6 | 87.23 → 89.82 | 10.1 → 10.1 |
| W3 | `20260829T160202Z-p3regate-w3` | 18 | 89.89 → 90.31 | 10.7 → 10.3 |
| sweep | `20260829T160338Z-p3regate-sweep` | 0 | 87.21 → 90.34 | 10.2 → 10.4 |
| a′ | `20260829T160447Z-p3regate-aprime` | 0 | 90.79 → 91.50 | 10.0 → 10.6 |
| AC-6/AC-1 | `20260829T160512Z-p3regate-ac6ac1` | 0 | 88.86 → 85.96 | 8.7 → 9.7 |
| reference | `20260829T161206Z-p3regate-ref` | 1 | 86.50 → 85.81 | 10.0 → 9.8 |

Discarded: three W2 attempts (`153206Z`, `153324Z`, `153422Z`) and eight W3
attempts (`154220Z`, `154320Z`, `154458Z`, `154557Z`, `155127Z`, `155229Z`,
`155532Z`, `155710Z`).

## The ensemble rule

A single 30-run group could not settle AC-3: clean readings of the same binary
on the same corpus spanned 134.9–140.9 ms against a 140.06 ms bound, so the
verdict depended on which half-hour the group ran in. The rule that replaces
it takes the same row in three separately gate-admitted windows and calls a
leg **MET** only when every window's own median clears the bound **and** the
pooled sample clears it, with the pooled IQR disjoint from the spike band for
the wall leg. Anything else is **AT-THE-BOUND**, reported with the numbers.

Pooling concatenates the windows' per-run wall times rather than averaging
their medians, so windows that disagree widen the pooled IQR instead of hiding
the disagreement. `user` and `system` are never pooled as a median: a
hyperfine export carries per-run wall times but only a per-group mean for CPU,
so the pooled CPU figure is the run-weighted mean.

Arithmetic in `scripts/bench-ensemble.py`, validated against `#21b`'s own
published figures before use — it reproduces 138.466 / 140.901 / 140.320 and
the 1.4568 system-time ratio, and returns AT-THE-BOUND on that sample, which
is the verdict the plan owner had reached by hand.

### a+b, `list_ab`, shipped default (`DEFAULT_THREADS = 4`), 30 runs per window

| Window | Median | IQR q1–q3 | User | Sys |
| --- | --- | --- | --- | --- |
| W1 `152228Z` | 137.449 | 134.914–139.911 | 37.563 | 474.631 |
| W2 `153917Z` | 145.790 | 143.146–150.077 | 39.433 | 507.097 |
| W3 `160202Z` | 146.828 | 140.675–153.483 | 39.765 | 512.220 |
| **Pooled (90)** | **143.326** | **137.931–148.926** | **38.920** | **497.983** |

The N=1 companions, taken in the same groups: 371.279 / 354.033 / 371.451 ms
wall, `sys` 344.498 / 327.785 / 345.835.

The row is spelled as the plain `list_ab` row with no `SCAP_LIST_THREADS`
override, because four threads is what the program ships and what a user runs.

### Verdicts

| Leg | Bound | W1 | W2 | W3 | Pooled | Verdict |
| --- | --- | --- | --- | --- | --- | --- |
| AC-3c wall | ≤ 140.06 | +2.611 | **−5.730** | **−6.768** | **−3.266** | **AT-THE-BOUND** |
| AC-3d wall | ≤ 140.54 | +3.091 | **−5.250** | **−6.288** | **−2.786** | **AT-THE-BOUND** |
| AC-3a user | ≤ 39.42 | +1.857 | **−0.013** | **−0.345** | +0.500 | **AT-THE-BOUND** |

The pooled IQR [137.931, 148.926] **is** disjoint from the spike band
154.44–160.90, so AC-3d's dispersion clause holds; only the median clause
fails. AC-3a's pooled figure passes and two of three windows fail, one of them
by **thirteen microseconds** — a margin no honest instrument resolves.

## Thread sweep

`20260829T160338Z-p3regate-sweep`, a+b on the shipped default, 30 runs per
point.

| N | Wall | Clause 1 (≤ 1.10 × best) | Sys | sys(N)/sys(1) | Clause 2 (≤ 1.5) |
| --- | --- | --- | --- | --- | --- |
| 1 | 358.040 | over | 332.030 | 1.0000 | met |
| 2 | 212.545 | over | 380.116 | 1.1448 | met |
| 4 | 142.013 | **over by 3.801** | 497.001 | **1.4969** | met |
| 8 | 125.648 | met | 852.978 | 2.5690 | **over by 354.933** |

Best wall is N=8's 125.648, so clause 1's window is 138.212 ms and N=4 misses
it; N=8 clears clause 1 and fails clause 2 against a 498.045 ms ceiling.
**No admissible `N`.** This reproduces the `#21b` REVISED sweep in shape, and
the four-thread system-time ratio sits **0.2 % under its ceiling**.

Clause 2 evaluated separately in the three ensemble windows: **1.3777 met,
1.5470 over, 1.4811 met — 2 of 3**, which satisfies the ensemble's ≥ 2
condition on its own. The rule fails on clause 1, not on system time.

As `#22`'s cross-check already observed, a "1.10 × best" window is ill-posed
where the 4-to-8 curve is flat: the clause *tightens* as N=8 improves. The
thread rule is **AT-THE-BOUND**; the deviation number for it is D-9 (D-8 is
taken by the AC-7 guard restatement).

## Same-window reference: the drift question, answered

`20260829T161206Z-p3regate-ref`. Rows alternate HEAD, reference, HEAD,
reference so that drift *within* the group is visible as a difference between
two readings of the same binary, instead of being attributed to whichever
binary ran second. Every binary was sha-verified before the group and confirmed
to print byte-identical output first: `074bbdd4` / 845 lines on corpus a and
`3a080dfa` / 1,826 lines on a+b, for all of them. They measure the same work.

| Row | Binary | Median | IQR | User | Sys |
| --- | --- | --- | --- | --- | --- |
| `ref_head_1` | HEAD `86dd2835` | 145.395 | 139.470–153.186 | 39.405 | 505.998 |
| `ref_w2b1_1` | Phase-2b end `0b48c24a` | 144.006 | 141.554–149.883 | 167.792 | 522.058 |
| `ref_head_2` | HEAD `86dd2835` | 153.587 | 148.698–157.957 | 40.843 | 532.109 |
| `ref_w2b1_2` | Phase-2b end `0b48c24a` | 144.029 | 139.891–149.549 | 167.097 | 524.535 |
| `ref_w12` | W1.2 `eb75a35e` | 141.322 | 139.333–147.988 | 165.622 | 515.346 |

**The bracket holds still.** The reference binary reads 144.006 and then
144.029 — **0.023 ms apart** across the whole group. The window did not drift.
HEAD, measured between those two readings, swings **8.192 ms** (145.395 then
153.587). The variance is HEAD's own, not the host's.

**The host has moved, against the frozen absolutes.** The same reference
binary recorded 135.358 ms in its own Phase-2b window and reads 144.017 today:
**+6.4 %**. Every §9 absolute therefore bites about 6 % harder than when it was
frozen. This corroborates `#21b`'s finding on a′ and is real.

**But drift does not rescue the wall criterion**, because the ratio form fails
too. AC-3d asks for ≤ 0.90 against the pre-Phase-3 walker:

| HEAD figure used | Value | ÷ reference 144.029 |
| --- | --- | --- |
| This group, 60 runs | 149.384 | **1.0372** |
| Ensemble pooled, 90 runs | 143.326 | **0.9951** |
| Best clean window (W1) | 137.449 | **0.9543** |

Every figure available today is far above 0.90. A bootstrap CI on this group's
60-versus-60 runs puts median(HEAD) − median(reference) at
**[+2.271, +10.046] ms**, excluding zero — though that group holds HEAD's
slowest reading of the day, so the defensible statement is the weaker and
robust one: **on this host today the new walker is not faster than the walker
it replaced in wall time, somewhere between parity and about 4 % slower.**

**The criterion Phase 3 was built to move is met.** User CPU against the same
live reference is **0.2396** in this group and **0.2324** using the ensemble's
pooled user figure, against the plan's 0.23 target. The new walker does the
same work for roughly a quarter of the user CPU; it does not convert that into
wall time on this machine.

### The W1.2 row, read with care

`eb75a35e` reads 141.322 today against the Phase-0 baseline's frozen 156.15,
which looks like the machine being 9.5 % *faster*, contradicting the +6.4 %
above. It does not: **these are different programs.** The plan names
`78b3212` as **W1.2 itself**, not a pre-Phase-1 commit, and W1.2 is the commit
that pinned the toolchain to `nightly-2026-08-28` where the baseline was built
on a floating nightly. The ratio mixes a compiler change with any host
movement and cannot settle drift on its own.

**The Phase-0 baseline binary (`fe0dc41e…`) is not preserved**, and none of the
three available AC-6-lane binaries predates Phase 1 — the plan names them as
W1.2 (`78b3212`), W1.3 (`21eb825`) and W1.4 (`6ab9038`). The row is published
as the closest available stand-in and nothing more.

## Corpus a′, AC-9, AC-6, AC-1

`20260829T160447Z-p3regate-aprime` and `20260829T160512Z-p3regate-ac6ac1`.

| Row | Median | IQR | User | Sys |
| --- | --- | --- | --- | --- |
| `list_aprime` | 12.814 | 12.282–13.544 | 4.379 | 23.630 |
| `empty` | 2.015 | 1.935–2.056 | 0.879 | 0.607 |
| `version` | 2.502 | 2.400–2.616 | 1.208 | 0.724 |
| `root_env` | 4.770 | 4.634–4.916 | 2.209 | 1.674 |
| `root_pinned_root_cwd` | 4.198 | 4.072–4.511 | 1.950 | 1.501 |
| `root_pinned_inside8_cwd` | 4.158 | 4.069–4.395 | 1.954 | 1.525 |

AC-3d(a′) carries on deviation D-6's **overlap arm**: 12.814 exceeds the
12.245 ms reference, and the IQRs overlap — [12.282, 13.544] against the
frozen [12.001, 12.714] — which is the arm D-6 wrote for a fixed-cost-dominated
corpus. The paired bootstrap CI, new minus frozen, is [+0.105, +1.178] ms, and
should be read beside the drift finding above: the frozen figure was recorded
on a machine that no longer reproduces.

## Acceptance table

| Criterion | Bound | Measured | Verdict |
| --- | --- | --- | --- |
| AC-3a a+b user | ≤ 39.42 | 37.563 / 39.433 / 39.765; pooled 38.920 | **AT-THE-BOUND** |
| AC-3c a+b wall | ≤ 140.06 | 137.449 / 145.790 / 146.828; pooled 143.326 | **AT-THE-BOUND** |
| AC-3d a+b wall | ≤ 140.54, IQR disjoint | pooled 143.326; IQR disjoint ✓ | **AT-THE-BOUND** |
| AC-3a a′ user | ≤ 12.224 | **4.379** | MET |
| AC-3d a′ wall | ≤ 12.245 or IQR overlap | 12.814, IQRs overlap | MET (overlap arm) |
| AC-9 a′ wall | ≤ 49.34 | 12.814 (3.85× margin) | MET |
| AC-9 `dirs_read` | ≤ 2,100 | **1,198** (stat-first default) | MET |
| AC-9 output equality | env == config | `074bbdd42b50dfe0` both ways | MET |
| AC-6 paired | ≤ 0.52 | **0.487** | MET |
| AC-6 absolute | ≤ 2.48 | 2.502 (over by 0.022) | **AT-THE-BOUND** |
| AC-1 `root_pinned_root_cwd` | ≤ 5.07 | 4.198 | MET |
| AC-1 `root_pinned_inside8_cwd` | ≤ 5.07 | 4.158 | MET |
| AC-1 `root_env` | ≤ 5.07 | 4.770 | MET |
| AC-4 thread identity | one hash per corpus | `074bbdd4` / `512c283d` / `3a080dfa` | MET |
| Thread rule `N*` | both clauses | no admissible N | **AT-THE-BOUND** |
| V-2 | no unsafe | none; forbidden in main.rs, denied in lib.rs and `[lints]` | MET |
| V-3 | no jwalk/rayon/fs2/dirs | none among 13 dependencies | MET |
| V-4 | empty diffs vs ghq | **15/15** (5 forms × 3 corpora) | MET |

AC-4 was taken over threads 1, 4 and 16 plus the default on each corpus: one
distinct stdout hash per corpus, and the three hashes are the same ones W3.4
and `#21b` recorded. The `074bbdd42b50dfe0` config-equals-env hash is now
continuous across four walkers.

AC-6's absolute leg missing by 0.022 ms repeats a dispersion the plan already
records as sitting at the measurement's resolution limit: `scap --version`
never walks, so Phase 3 cannot have moved it.

## Corpus inventory

Recorded, never gated. Corpus a: 845 repositories against the frozen 841,
**drift +4**, 16,933 directory reads. Corpus b: 981 repositories, drift 0,
2,608 reads. Corpus a′: 845 repositories, 1,198 reads, 1 excluded. Identical
across every group in the window, so no group measured a different corpus from
any other.

`dirs_read` is not comparable across walkers or across detection strategies;
`repos` is the drift signal.

## Gates

Run after the last timed row, on the dev profile with `RUSTFLAGS` unset:
`cargo fmt --check` clean; `cargo clippy --all-targets --all-features -D
warnings` zero warnings; `cargo nextest run --all-features` **341 passed, 0
skipped** — the same count `#21b` recorded; `cargo test --doc` ok. Binary
re-verified as `86dd2835` afterwards.

## Run directories

`20260829T152228Z-p3regate-w1`, `20260829T153917Z-p3regate-w2`,
`20260829T160202Z-p3regate-w3`, `20260829T160338Z-p3regate-sweep`,
`20260829T160447Z-p3regate-aprime`, `20260829T160512Z-p3regate-ac6ac1`,
`20260829T161206Z-p3regate-ref`.

Row definitions: `docs/benchmarks/extra-rows-phase-3.sh` (ensemble and sweep),
`docs/benchmarks/extra-rows-w2b1.sh` (a′),
`docs/benchmarks/extra-rows-p3-regate.sh` (reference group).
