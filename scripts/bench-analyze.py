#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Turn the Phase-0 hyperfine exports into the baseline document's tables.

Reads the run directories written by scripts/bench-quiet.sh and the W0.2/W0.3/
W0.5 spike drivers, and emits Markdown plus the derived verdicts the plan's
Phase 0 owes W0.6: the per-variant thread count `N*` from the §2 Decision B
thread-selection rule, the B2 adoption rule, the W3.0 walker gate, the ADR-10
index gate, and AC-7's `rm` arithmetic.

Statistics come from scripts/bench-compare.py (`load_hyperfine_json`,
`iqr_disjoint`, `bootstrap_median_diff_ci`), so both scripts agree on how a
hyperfine export becomes a median, an IQR box and a bootstrap interval.

hyperfine reports per-run wall times but only mean `user`/`system`, so every
wall figure below is a median over 30 runs while every CPU figure is a mean.
"""

from __future__ import annotations

import argparse
import importlib.util
import sys
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Protocol

_HERE = Path(__file__).resolve().parent


class Stats(Protocol):
    """The millisecond statistics bench-compare.py's `BenchStats` exposes.

    bench-compare.py cannot be imported by name (its filename is not a Python
    identifier), so it is loaded through importlib and its dataclass is
    described structurally here rather than imported as a type.
    """

    mean_ms: float
    stddev_ms: float
    median_ms: float
    q1_ms: float
    q3_ms: float
    min_ms: float
    max_ms: float
    user_ms: float
    system_ms: float
    times_ms: list[float]


def _load_bench_compare() -> ModuleType:
    """Import scripts/bench-compare.py as a module (its name is not an identifier)."""
    # Keep a __pycache__ directory out of scripts/: this is a one-shot analysis
    # tool and the cache would only ever be an untracked build artifact.
    sys.dont_write_bytecode = True
    path = _HERE / "bench-compare.py"
    spec = importlib.util.spec_from_file_location("bench_compare", path)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    # `@dataclass(slots=True)` rebuilds the class and looks its module up in
    # sys.modules, so the entry must exist before the module body runs.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


_bc = _load_bench_compare()


def load_stats(path: Path) -> Stats:
    """Load one hyperfine export through bench-compare.py's parser."""
    return _bc.load_hyperfine_json(path)


def iqr_disjoint(a: Stats, b: Stats) -> bool:
    """True iff the two [q1, q3] boxes do not overlap (bench-compare.py's test)."""
    return _bc.iqr_disjoint(a, b)


def bootstrap_ci(a: Stats, b: Stats) -> tuple[float, float]:
    """95 % bootstrap interval for median(b) - median(a), in ms."""
    return _bc.bootstrap_median_diff_ci(a, b)


VARIANTS = ("b1-lifo", "b1-fifo", "b1-deque", "b2-rustix", "b3-jwalk", "b4-attrbulk")
THREADS = (1, 2, 4, 8, 12, 16)
CORPORA = ("a", "b", "ab", "aprime")
CORPUS_LABEL = {"a": "a", "b": "b", "ab": "a+b", "aprime": "a'"}


@dataclass(frozen=True, slots=True)
class ThreadPoint:
    """One (variant, thread count) point and how it scores against the rule."""

    wall_ms: float
    sys_ms: float
    user_ms: float
    wall_ok: bool
    sys_ok: bool


@dataclass(frozen=True, slots=True)
class ThreadRule:
    """The outcome of the §2 Decision B thread-selection rule for one variant."""

    n: int | None
    min_wall_ms: float
    wall_bound_ms: float
    sys1_ms: float | None
    sys_budget_ms: float | None
    detail: dict[int, ThreadPoint]


def cpu_ms(stats: Stats) -> float:
    """Total CPU of a row: hyperfine's mean user plus mean system, in ms."""
    return stats.user_ms + stats.system_ms


def fmt_row(label: str, s: Stats) -> str:
    """Format one benchmark as the baseline document's seven-column row."""
    return (
        f"| {label} | {s.mean_ms:.2f} +/- {s.stddev_ms:.2f} | {s.median_ms:.2f} "
        f"| {s.q1_ms:.2f}-{s.q3_ms:.2f} | {s.min_ms:.2f} "
        f"| {s.user_ms:.2f} | {s.system_ms:.2f} |"
    )


def load_dir(run_dir: Path) -> dict[str, Stats]:
    """Load every single-result hyperfine export in a directory, keyed by stem."""
    out: dict[str, Stats] = {}
    for json_path in sorted(run_dir.glob("*.json")):
        if json_path.name == "metadata.json":
            continue
        try:
            out[json_path.stem] = load_stats(json_path)
        except (ValueError, KeyError):
            continue
    return out


# ---------------------------------------------------------------------------
# W0.2 thread rule and verdicts
# ---------------------------------------------------------------------------


def n_star(rows: dict[str, Stats], variant: str, corpus: str = "ab") -> ThreadRule:
    """Apply the plan §2 Decision B thread-selection rule to one variant.

    The rule, verbatim: on corpus a+b, `N* = smallest N in {1, 2, 4, 8, 12, 16}`
    such that `median wall(N) <= 1.10 x min_N median wall(N)` subject to
    `sys(N) <= 1.5 x sys(1)`.

    Args:
        rows: Benchmarks keyed by `w02-<variant>-t<threads>-<corpus>`.
        variant: Walker variant name.
        corpus: Corpus key the rule is applied on (a+b by the plan).

    Returns:
        The chosen `n` (None when no thread count qualifies) together with the
        bounds and per-thread detail the choice was made from.
    """
    per_n: dict[int, Stats] = {}
    for t in THREADS:
        s = rows.get(f"w02-{variant}-t{t}-{corpus}")
        if s is not None:
            per_n[t] = s
    if not per_n:
        return ThreadRule(None, float("nan"), float("nan"), None, None, {})

    min_wall = min(s.median_ms for s in per_n.values())
    wall_bound = 1.10 * min_wall
    sys1 = per_n[1].system_ms if 1 in per_n else None
    sys_budget = 1.5 * sys1 if sys1 is not None else None

    detail: dict[int, ThreadPoint] = {}
    chosen: int | None = None
    for t in sorted(per_n):
        s = per_n[t]
        wall_ok = s.median_ms <= wall_bound
        sys_ok = sys_budget is None or s.system_ms <= sys_budget
        detail[t] = ThreadPoint(s.median_ms, s.system_ms, s.user_ms, wall_ok, sys_ok)
        if chosen is None and wall_ok and sys_ok:
            chosen = t
    return ThreadRule(chosen, min_wall, wall_bound, sys1, sys_budget, detail)


def cmd_w02(args: argparse.Namespace) -> int:
    """Emit the W0.2 matrix table, `N*` per variant, and the Decision B verdicts."""
    rows = load_dir(Path(args.run_dir))

    print("### W0.2 walker matrix\n")
    print(
        "| Variant | Threads | Corpus | Median wall (ms) | IQR q1-q3 (ms) | "
        "Mean +/- sigma (ms) | Min (ms) | User (ms) | Sys (ms) |"
    )
    print("| --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    for v in VARIANTS:
        for c in CORPORA:
            for t in THREADS:
                s = rows.get(f"w02-{v}-t{t}-{c}")
                if s is None:
                    continue
                print(
                    f"| `{v}` | {t} | {CORPUS_LABEL[c]} | {s.median_ms:.2f} "
                    f"| {s.q1_ms:.2f}-{s.q3_ms:.2f} "
                    f"| {s.mean_ms:.2f} +/- {s.stddev_ms:.2f} | {s.min_ms:.2f} "
                    f"| {s.user_ms:.2f} | {s.system_ms:.2f} |"
                )

    stars = {v: n_star(rows, v) for v in VARIANTS}

    print("\n#### `N*` per variant (thread rule, corpus a+b)\n")
    print(
        "| Variant | `N*` | min median wall (ms) | 1.10x bound (ms) | sys(1) (ms) | "
        "1.5x sys budget (ms) | wall(N*) (ms) | sys(N*) (ms) | user(N*) (ms) |"
    )
    print("| --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    for v in VARIANTS:
        r = stars[v]
        if r.n is None or r.sys1_ms is None or r.sys_budget_ms is None:
            print(f"| `{v}` | none | | | | | | | |")
            continue
        d = r.detail[r.n]
        print(
            f"| `{v}` | {r.n} | {r.min_wall_ms:.2f} | {r.wall_bound_ms:.2f} "
            f"| {r.sys1_ms:.2f} | {r.sys_budget_ms:.2f} "
            f"| {d.wall_ms:.2f} | {d.sys_ms:.2f} | {d.user_ms:.2f} |"
        )

    print("\n#### Thread-rule detail (a+b): which N pass each clause\n")
    print("| Variant | " + " | ".join(f"N={t}" for t in THREADS) + " |")
    print("| --- | " + " | ".join("---" for _ in THREADS) + " |")
    for v in VARIANTS:
        cells = []
        for t in THREADS:
            d = stars[v].detail.get(t)
            if d is None:
                cells.append("-")
                continue
            marks = ("W" if d.wall_ok else "w") + ("S" if d.sys_ok else "s")
            cells.append(f"{d.wall_ms:.0f}/{d.sys_ms:.0f} {marks}")
        print(f"| `{v}` | " + " | ".join(cells) + " |")
    print(
        "\nCell = median wall / sys in ms, then flags: `W` wall within the 1.10x "
        "bound (`w` outside), `S` sys within 1.5x sys(1) (`s` outside).\n"
    )

    print(
        "#### Scheduler A/B (H1 vs H2): sys inflation vs each arm's own sys(1), a+b\n"
    )
    print("| Variant | " + " | ".join(f"N={t}" for t in THREADS) + " |")
    print("| --- | " + " | ".join("---" for _ in THREADS) + " |")
    for v in VARIANTS:
        r = stars[v]
        cells = []
        for t in THREADS:
            d = r.detail.get(t)
            if d is None or not r.sys1_ms:
                cells.append("-")
            else:
                cells.append(f"{d.sys_ms / r.sys1_ms:.2f}x")
        print(f"| `{v}` | " + " | ".join(cells) + " |")

    print("\n#### Decision B adoption rules (corpus a+b)\n")
    baseline_variant = "b1-deque"
    n_b1 = stars[baseline_variant].n
    s_b1 = rows.get(f"w02-{baseline_variant}-t{n_b1}-ab") if n_b1 else None
    if s_b1 is not None:
        for other, rule in (
            ("b2-rustix", "B2 adoption rule"),
            ("b4-attrbulk", "B4 vs b1-deque (RUN.md §8)"),
        ):
            n_ot = stars[other].n
            s_own = rows.get(f"w02-{other}-t{n_ot}-ab") if n_ot else None
            if s_own is None:
                continue
            print(
                f"- **{rule}** - `{other}` at its own N*={n_ot} vs `{baseline_variant}` "
                f"at N*={n_b1}: CPU {cpu_ms(s_own):.1f} ms vs {cpu_ms(s_b1):.1f} ms = "
                f"{cpu_ms(s_own) / cpu_ms(s_b1):.3f}x (user {s_own.user_ms:.1f} vs "
                f"{s_b1.user_ms:.1f}; sys {s_own.system_ms:.1f} vs {s_b1.system_ms:.1f}; "
                f"wall {s_own.median_ms:.1f} vs {s_b1.median_ms:.1f} ms)"
            )
            s_match = rows.get(f"w02-{other}-t{n_b1}-ab")
            if s_match is not None:
                print(
                    f"  - at matched N={n_b1}: CPU {cpu_ms(s_match):.1f} ms = "
                    f"{cpu_ms(s_match) / cpu_ms(s_b1):.3f}x (user {s_match.user_ms:.1f}, "
                    f"sys {s_match.system_ms:.1f}, wall {s_match.median_ms:.1f} ms)"
                )

    print(
        "\n#### W3.0 gate inputs (each variant at its `N*` vs `D_wall` = "
        "`b3-jwalk` at its own `N*`)\n"
    )
    n_b3 = stars["b3-jwalk"].n
    print(
        "| Corpus | Variant | N | Median wall (ms) | IQR (ms) | User (ms) | Sys (ms) | "
        "CPU (ms) | IQR disjoint vs D | 95% CI of median(variant)-median(D) (ms) |"
    )
    print("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    for c in ("aprime", "ab"):
        d_row = rows.get(f"w02-b3-jwalk-t{n_b3}-{c}") if n_b3 else None
        for v in VARIANTS:
            n = stars[v].n
            s = rows.get(f"w02-{v}-t{n}-{c}") if n else None
            if s is None:
                continue
            if d_row is None or v == "b3-jwalk":
                disj, ci = "-", "-"
            else:
                disj = "yes" if iqr_disjoint(s, d_row) else "no"
                lo, hi = bootstrap_ci(d_row, s)
                ci = f"[{lo:.2f}, {hi:.2f}] " + (
                    "excludes 0" if not (lo <= 0 <= hi) else "includes 0"
                )
            print(
                f"| {CORPUS_LABEL[c]} | `{v}` | {n} | {s.median_ms:.2f} "
                f"| {s.q1_ms:.2f}-{s.q3_ms:.2f} | {s.user_ms:.2f} | {s.system_ms:.2f} "
                f"| {cpu_ms(s):.2f} | {disj} | {ci} |"
            )
    return 0


# ---------------------------------------------------------------------------
# Baseline, W0.5, W0.3
# ---------------------------------------------------------------------------


def cmd_baseline(args: argparse.Namespace) -> int:
    """Emit the W0.1 baseline rows table."""
    rows = load_dir(Path(args.run_dir))
    print(
        "| Row | Mean +/- sigma (ms) | Median (ms) | IQR q1-q3 (ms) | Min (ms) | "
        "User (ms) | Sys (ms) |"
    )
    print("| --- | --- | --- | --- | --- | --- | --- |")
    for name in sorted(rows):
        print(fmt_row(f"`{name}`", rows[name]))
    return 0


def cmd_w05(args: argparse.Namespace) -> int:
    """Emit the W0.5 table and, given a shipped walker, the ADR-10 gate arithmetic."""
    rows = load_dir(Path(args.run_dir))
    print(
        "| Corpus | Threads | Median wall (ms) | IQR q1-q3 (ms) | User (ms) | "
        "Sys (ms) | CPU (ms) |"
    )
    print("| --- | --- | --- | --- | --- | --- | --- |")
    for c in ("a", "aprime"):
        for n in (1, 4, 8, 16):
            s = rows.get(f"w05-stat-{c}-n{n}")
            if s is None:
                continue
            print(
                f"| {CORPUS_LABEL[c]} | {n} | {s.median_ms:.2f} "
                f"| {s.q1_ms:.2f}-{s.q3_ms:.2f} | {s.user_ms:.2f} "
                f"| {s.system_ms:.2f} | {cpu_ms(s):.2f} |"
            )
    if args.r_wall is not None and args.r_cpu is not None and args.n_star is not None:
        s = rows.get(f"w05-stat-aprime-n{args.n_star}")
        if s is not None:
            wall_ok = s.median_ms <= 0.5 * args.r_wall
            cpu_ok = cpu_ms(s) <= 0.5 * args.r_cpu
            print(
                f"\nADR-10 gate on a' at N={args.n_star}: "
                f"I_wall={s.median_ms:.2f} ms vs 0.5 x R_wall={0.5 * args.r_wall:.2f} ms "
                f"-> {'PASS' if wall_ok else 'FAIL'}; "
                f"I_cpu={cpu_ms(s):.2f} ms vs 0.5 x R_cpu={0.5 * args.r_cpu:.2f} ms "
                f"-> {'PASS' if cpu_ok else 'FAIL'}. "
                f"ADR-10 {'ADOPTED' if wall_ok and cpu_ok else 'REJECTED'}."
            )
    return 0


def _rmspike_cpu(cpu_log: Path) -> dict[str, dict[str, float]]:
    """Median wall/user/sys per arm from rmspike's own getrusage lines."""
    buckets: dict[str, dict[str, list[float]]] = {}
    for line in cpu_log.read_text().splitlines():
        fields = dict(kv.split("=", 1) for kv in line.split() if "=" in kv)
        if "mode" not in fields or "threads" not in fields:
            continue
        arm = "std" if fields["mode"] == "std" else f"pool@{fields['threads']}"
        b = buckets.setdefault(arm, {"wall_ms": [], "user_ms": [], "sys_ms": []})
        for key in ("wall_ms", "user_ms", "sys_ms"):
            if key in fields:
                b[key].append(float(fields[key]))
    return {
        arm: {k: sorted(v)[len(v) // 2] for k, v in b.items() if v}
        for arm, b in buckets.items()
    }


def cmd_w03(args: argparse.Namespace) -> int:
    """Emit the W0.3 rm arms and AC-7's Decision D-4 arithmetic."""
    run_dir = Path(args.run_dir)
    rows: dict[str, Stats] = {}
    for arm, stem in (
        ("std", "w03-rm-std"),
        ("pool@4", "w03-rm-pool_at_4"),
        ("pool@8", "w03-rm-pool_at_8"),
    ):
        path = run_dir / f"{stem}.json"
        if path.exists():
            rows[arm] = load_stats(path)

    cpu_log = run_dir / "w03-rm-cpu.log"
    own = _rmspike_cpu(cpu_log) if cpu_log.exists() else {}

    std = rows.get("std")
    std_cpu = own.get("std", {}).get("user_ms", 0.0) + own.get("std", {}).get(
        "sys_ms", 0.0
    )

    print(
        "| Arm | wall median ms | wall IQR ms | user ms | sys ms | CPU ms | "
        "wall vs std | CPU vs std |"
    )
    print("| --- | --- | --- | --- | --- | --- | --- | --- |")
    for arm in ("std", "pool@4", "pool@8"):
        s = rows.get(arm)
        if s is None:
            continue
        o = own.get(arm, {})
        cpu = o.get("user_ms", 0.0) + o.get("sys_ms", 0.0)
        wall_ratio = s.median_ms / std.median_ms if std else float("nan")
        cpu_ratio = cpu / std_cpu if std_cpu else float("nan")
        print(
            f"| `{arm}` | {s.median_ms:.2f} | {s.q1_ms:.2f}-{s.q3_ms:.2f} "
            f"| {o.get('user_ms', float('nan')):.2f} | {o.get('sys_ms', float('nan')):.2f} "
            f"| {cpu:.2f} | {wall_ratio:.3f} | {cpu_ratio:.3f} |"
        )

    p8 = rows.get("pool@8")
    if p8 is not None and std is not None:
        cpu8 = own.get("pool@8", {}).get("user_ms", 0.0) + own.get("pool@8", {}).get(
            "sys_ms", 0.0
        )
        wall_ok = p8.median_ms <= 0.67 * std.median_ms
        cpu_ok = std_cpu > 0 and cpu8 <= 1.30 * std_cpu
        print(
            f"\nAC-7 at 8 threads: wall {p8.median_ms:.2f} <= 0.67 x {std.median_ms:.2f} "
            f"= {0.67 * std.median_ms:.2f} -> {'PASS' if wall_ok else 'FAIL'}; "
            f"CPU {cpu8:.2f} <= 1.30 x {std_cpu:.2f} = {1.30 * std_cpu:.2f} "
            f"-> {'PASS' if cpu_ok else 'FAIL'}. "
            f"D-4 {'ADOPTED' if wall_ok and cpu_ok else 'REJECTED'}."
        )
    return 0


def build_parser() -> argparse.ArgumentParser:
    """Build the CLI parser."""
    parser = argparse.ArgumentParser(prog="bench-analyze.py", description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("baseline", help="W0.1 baseline rows table.")
    p.add_argument("run_dir")
    p.set_defaults(func=cmd_baseline)

    p = sub.add_parser("w02", help="W0.2 matrix, N* per variant, Decision B verdicts.")
    p.add_argument("run_dir")
    p.set_defaults(func=cmd_w02)

    p = sub.add_parser("w05", help="W0.5 table and the ADR-10 gate.")
    p.add_argument("run_dir")
    p.add_argument(
        "--r-wall",
        type=float,
        default=None,
        help="Shipped walker median wall on a' (ms).",
    )
    p.add_argument(
        "--r-cpu",
        type=float,
        default=None,
        help="Shipped walker CPU (user+sys) on a' (ms).",
    )
    p.add_argument(
        "--n-star",
        type=int,
        default=None,
        help="Thread count the index gate is read at.",
    )
    p.set_defaults(func=cmd_w05)

    p = sub.add_parser("w03", help="W0.3 rm arms and AC-7 arithmetic.")
    p.add_argument("run_dir")
    p.set_defaults(func=cmd_w03)

    return parser


def main(argv: list[str] | None = None) -> int:
    """CLI entry point."""
    args = build_parser().parse_args(argv)
    return int(args.func(args))


if __name__ == "__main__":
    sys.exit(main())
