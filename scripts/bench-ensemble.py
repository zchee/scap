#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Evaluate an acceptance leg across several benchmark windows at once.

The Phase-3 re-gate exists because a single 30-run group could not settle
AC-3: repeated clean readings of the same program on the same corpus spanned
134.9 to 140.9 ms against a 140.06 ms bound, so whether the criterion "passed"
depended on which half-hour the group happened to run in.  A verdict that a
re-run can flip is not a verdict.

This script evaluates the ensemble rule that replaces it.  The same row is
taken in three separate gate-admitted windows, and a leg is MET only when
every window's own median clears the bound *and* the pooled sample clears it
too; anything else is AT-THE-BOUND, reported with the numbers rather than
rounded into a pass or a failure.  Bounds are inputs here and are never
adjusted to fit a sample: relaxing one is the plan owner's decision, not this
tool's and not a measuring lane's.

Pooling concatenates the windows' per-run wall times rather than averaging
their medians, so the pooled IQR widens honestly when the windows disagree
instead of hiding the disagreement in a mean of medians.  hyperfine reports
per-run wall times but only mean user/system figures per group, so user and
system are reported per window and, when pooled, as the run-weighted mean --
never as a median, which the export does not support.

Usage:

    scripts/bench-ensemble.py wall \\
        --bound 140.06 --disjoint-from 154.44:160.90 \\
        w1/list_ab.json w2/list_ab.json w3/list_ab.json

    scripts/bench-ensemble.py user --bound 39.42 w1/... w2/... w3/...

    scripts/bench-ensemble.py sys-ratio --bound 1.5 \\
        --pair w1/list_ab_t1.json:w1/list_ab.json \\
        --pair w2/list_ab_t1.json:w2/list_ab.json \\
        --pair w3/list_ab_t1.json:w3/list_ab.json
"""

from __future__ import annotations

import argparse
import importlib.util
import statistics
import sys
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType

_HERE = Path(__file__).resolve().parent


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


_BC = _load_bench_compare()


def _quantile(sorted_values: list[float], q: float) -> float:
    """Linearly-interpolated quantile, identical to bench-compare.py's."""
    if not sorted_values:
        raise ValueError("no values")
    if len(sorted_values) == 1:
        return sorted_values[0]
    h = q * (len(sorted_values) - 1)
    lo = int(h)
    hi = min(lo + 1, len(sorted_values) - 1)
    return sorted_values[lo] + (h - lo) * (sorted_values[hi] - sorted_values[lo])


@dataclass(frozen=True, slots=True)
class Window:
    """One gate-admitted measurement window's reading of a single row."""

    label: str
    runs: int
    median_ms: float
    q1_ms: float
    q3_ms: float
    user_ms: float
    system_ms: float
    times_ms: list[float]


def _window(path: Path) -> Window:
    stats = _BC.load_hyperfine_json(path)
    # The run directory names the window; the file names the row.
    return Window(
        label=f"{path.parent.name}/{path.stem}",
        runs=len(stats.times_ms),
        median_ms=stats.median_ms,
        q1_ms=stats.q1_ms,
        q3_ms=stats.q3_ms,
        user_ms=stats.user_ms,
        system_ms=stats.system_ms,
        times_ms=list(stats.times_ms),
    )


def _pooled(windows: list[Window]) -> tuple[int, float, float, float, float, float]:
    """Return (runs, median, q1, q3, user, system) over the concatenated windows."""
    times = sorted(t for w in windows for t in w.times_ms)
    runs = sum(w.runs for w in windows)
    # user/system are per-group means in a hyperfine export, so the pooled
    # figure is the run-weighted mean of them, not a median of run values that
    # the export never recorded.
    user = sum(w.user_ms * w.runs for w in windows) / runs
    system = sum(w.system_ms * w.runs for w in windows) / runs
    return (
        runs,
        statistics.median(times),
        _quantile(times, 0.25),
        _quantile(times, 0.75),
        user,
        system,
    )


def _verdict(all_windows_met: bool, pooled_met: bool, disjoint_met: bool) -> str:
    return "MET" if (all_windows_met and pooled_met and disjoint_met) else "AT-THE-BOUND"


def _print_table(rows: list[tuple[str, ...]], header: tuple[str, ...]) -> None:
    widths = [max(len(h), *(len(r[i]) for r in rows)) for i, h in enumerate(header)]
    line = "| " + " | ".join(h.ljust(widths[i]) for i, h in enumerate(header)) + " |"
    print(line)
    print("| " + " | ".join("-" * widths[i] for i in range(len(header))) + " |")
    for row in rows:
        print("| " + " | ".join(row[i].ljust(widths[i]) for i in range(len(header))) + " |")


def _metric_value(window: Window, metric: str) -> float:
    return {
        "wall": window.median_ms,
        "user": window.user_ms,
        "sys": window.system_ms,
    }[metric]


def cmd_metric(args: argparse.Namespace) -> int:
    """Evaluate one <= bound leg over N windows plus their pooled sample."""
    metric = args.metric
    windows = [_window(Path(p)) for p in args.exports]
    if len(windows) < 2:
        print("bench-ensemble: an ensemble needs at least two windows", file=sys.stderr)
        return 2

    bound = args.bound
    rows: list[tuple[str, ...]] = []
    per_window_met = True
    for w in windows:
        value = _metric_value(w, metric)
        met = value <= bound
        per_window_met = per_window_met and met
        rows.append(
            (
                w.label,
                str(w.runs),
                f"{w.median_ms:.3f}",
                f"{w.q1_ms:.3f}-{w.q3_ms:.3f}",
                f"{w.user_ms:.3f}",
                f"{w.system_ms:.3f}",
                f"{value:.3f}",
                f"{bound - value:+.3f}",
                "met" if met else "OVER",
            )
        )

    runs, median, q1, q3, user, system = _pooled(windows)
    pooled_value = {"wall": median, "user": user, "sys": system}[metric]
    pooled_met = pooled_value <= bound
    rows.append(
        (
            "POOLED",
            str(runs),
            f"{median:.3f}",
            f"{q1:.3f}-{q3:.3f}",
            f"{user:.3f}",
            f"{system:.3f}",
            f"{pooled_value:.3f}",
            f"{bound - pooled_value:+.3f}",
            "met" if pooled_met else "OVER",
        )
    )
    _print_table(
        rows,
        ("window", "runs", "median", "IQR q1-q3", "user", "sys", metric, "margin", ""),
    )

    disjoint_met = True
    if args.disjoint_from:
        lo, hi = (float(x) for x in args.disjoint_from.split(":"))
        # Disjoint means the pooled box lies wholly outside the reference band,
        # on either side; the gate cares that the two are separable, not which
        # way round they sit.
        disjoint_met = q3 < lo or q1 > hi
        print(
            f"\npooled IQR [{q1:.3f}, {q3:.3f}] vs reference band [{lo:.2f}, {hi:.2f}]: "
            f"{'DISJOINT' if disjoint_met else 'OVERLAPPING'}"
        )

    verdict = _verdict(per_window_met, pooled_met, disjoint_met)
    print(f"\n{metric} <= {bound}: {verdict}")
    if verdict != "MET":
        for w in windows:
            value = _metric_value(w, metric)
            if value > bound:
                print(f"  over in {w.label}: {value:.3f} exceeds {bound} by {value - bound:.3f}")
        if not pooled_met:
            print(
                f"  over pooled: {pooled_value:.3f} exceeds {bound} by {pooled_value - bound:.3f}"
            )
        if not disjoint_met:
            print("  pooled IQR is not disjoint from the reference band")
    # A verdict is a report, not an exit status: AT-THE-BOUND is an outcome to
    # be ruled on, not a tool failure, so both verdicts exit 0 and only a
    # malformed invocation is an error.
    return 0


def cmd_sys_ratio(args: argparse.Namespace) -> int:
    """Report sys(N)/sys(1) per window against the thread rule's ceiling."""
    rows: list[tuple[str, ...]] = []
    met_count = 0
    for pair in args.pair:
        one, many = pair.split(":", 1)
        w1 = _window(Path(one))
        wn = _window(Path(many))
        ratio = wn.system_ms / w1.system_ms
        met = ratio <= args.bound
        met_count += int(met)
        rows.append(
            (
                w1.label.split("/")[0],
                f"{w1.system_ms:.3f}",
                f"{wn.system_ms:.3f}",
                f"{ratio:.4f}",
                f"{(args.bound - ratio) / args.bound * 100:+.2f} %",
                "met" if met else "OVER",
            )
        )
    _print_table(rows, ("window", "sys(1)", "sys(N)", "ratio", "margin", ""))
    # The clause is reported per window because the last clean reading sat 0.3 %
    # under the ceiling: an ensemble that says "2 of 3" is the honest summary of
    # a ratio that close, and the ruling on it belongs to the plan owner.
    print(f"\nsys(N)/sys(1) <= {args.bound}: met in {met_count} of {len(rows)} windows")
    return 0


def build_parser() -> argparse.ArgumentParser:
    # `__doc__` is None under `python -OO`, so the summary line is spelled out
    # rather than sliced out of it.
    parser = argparse.ArgumentParser(
        description="Evaluate an acceptance leg across several benchmark windows at once."
    )
    sub = parser.add_subparsers(dest="command", required=True)

    for metric in ("wall", "user", "sys"):
        p = sub.add_parser(metric, help=f"evaluate the {metric} leg across windows")
        p.add_argument("exports", nargs="+", help="one hyperfine JSON per window")
        p.add_argument("--bound", type=float, required=True, help="the frozen upper bound (ms)")
        if metric == "wall":
            # Only on `wall`: the pooled IQR is a box of per-run WALL times, so
            # asking whether it clears a reference band is a question only the
            # wall leg can answer. Offering the flag on the CPU legs would let a
            # run report a disjointness verdict about a quantity it never
            # measured, and this gate has already been misread once.
            p.add_argument(
                "--disjoint-from",
                metavar="LO:HI",
                help="reference band the pooled wall IQR must not overlap, e.g. 154.44:160.90",
            )
        else:
            p.set_defaults(disjoint_from=None)
        p.set_defaults(func=cmd_metric, metric=metric)

    p = sub.add_parser("sys-ratio", help="thread rule clause 2: sys(N)/sys(1) per window")
    p.add_argument(
        "--pair",
        action="append",
        required=True,
        metavar="ONE.json:MANY.json",
        help="the N=1 export and the N=N* export from the same window",
    )
    p.add_argument("--bound", type=float, default=1.5, help="the clause ceiling (default 1.5)")
    p.set_defaults(func=cmd_sys_ratio)

    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    return int(args.func(args))


if __name__ == "__main__":
    raise SystemExit(main())
