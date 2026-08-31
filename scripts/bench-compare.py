#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Compare hyperfine --export-json results against §9-style thresholds.

Loads one or more hyperfine JSON exports (one benchmark result per file, the
shape scripts/bench-quiet.sh writes), converts them to millisecond
statistics, and evaluates a thresholds file describing pass/fail criteria of
the kind plan §9's acceptance criteria use: an absolute margin over a
reference, a ratio against a reference, non-overlapping IQR boxes, or a
bootstrap confidence interval on the median difference.

The two separation criteria ("iqr", "bootstrap") require an explicit
"direction" field, because separation on its own is symmetric and a criterion
that reads "no slower than" must not pass on a build that is significantly
slower. See _DIRECTIONS. An empty thresholds file, or one missing a
--require'd criterion, exits 2 rather than reporting a vacuous pass.
"""

from __future__ import annotations

import argparse
import json
import random
import statistics
import sys
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True, slots=True)
class BenchStats:
    """Millisecond-scaled statistics parsed from one hyperfine JSON result."""

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


def _quantile(sorted_values: list[float], q: float) -> float:
    """Compute a linearly-interpolated quantile (numpy's default method).

    Args:
        sorted_values: Values sorted ascending.
        q: Quantile in [0, 1].

    Returns:
        The interpolated value at quantile q.
    """
    n = len(sorted_values)
    if n == 1:
        return sorted_values[0]
    h = q * (n - 1)
    lo = int(h)
    hi = min(lo + 1, n - 1)
    frac = h - lo
    return sorted_values[lo] + frac * (sorted_values[hi] - sorted_values[lo])


def load_hyperfine_json(path: Path) -> BenchStats:
    """Load a hyperfine --export-json result and convert seconds to ms.

    Args:
        path: Path to a hyperfine JSON export with exactly one result.

    Returns:
        Parsed benchmark statistics in milliseconds.

    Raises:
        ValueError: If the file holds zero or more than one result.
    """
    data = json.loads(path.read_text())
    results = data.get("results", [])
    if len(results) != 1:
        raise ValueError(f"{path}: expected exactly one result, got {len(results)}")
    r = results[0]
    times_ms = sorted(t * 1000.0 for t in r["times"])
    return BenchStats(
        mean_ms=r["mean"] * 1000.0,
        stddev_ms=r["stddev"] * 1000.0,
        median_ms=r["median"] * 1000.0,
        q1_ms=_quantile(times_ms, 0.25),
        q3_ms=_quantile(times_ms, 0.75),
        min_ms=r["min"] * 1000.0,
        max_ms=r["max"] * 1000.0,
        user_ms=r["user"] * 1000.0,
        system_ms=r["system"] * 1000.0,
        times_ms=times_ms,
    )


def iqr_disjoint(a: BenchStats, b: BenchStats) -> bool:
    """Return True iff a's and b's [q1, q3] boxes do not overlap."""
    return a.q3_ms < b.q1_ms or b.q3_ms < a.q1_ms


#: Accepted values of a criterion's "direction" field.
#:
#: "iqr" and "bootstrap" answer "are these two distributions separated?", which
#: is a two-sided question, while every criterion the plan states is one-sided:
#: a bound not to exceed, or a build that must be no slower than its
#: predecessor. Reading a two-sided answer as a one-sided verdict is what let a
#: significant REGRESSION report PASS -- non-overlapping boxes and a CI that
#: excludes zero are exactly as true when lhs is the slower side.
#:
#: There is deliberately no default. A thresholds file written before W5.4 does
#: not record which side it expected to win, so guessing on its behalf would
#: silently assign a meaning to a file that never had one; such a file now
#: fails with an explicit error instead.
_DIRECTIONS = {
    # lhs must be significantly FASTER (lower) than rhs.
    "lhs_faster",
    # rhs must be significantly faster than lhs.
    "rhs_faster",
    # The pre-W5.4 two-sided reading: the two only have to be separated, in
    # either direction. Only honest for a criterion that genuinely asks
    # "did anything change at all?".
    "differ",
}


def _require_direction(spec: dict[str, object]) -> str:
    """Read and validate a criterion's required "direction" field.

    Args:
        spec: One criterion from the thresholds file.

    Returns:
        The validated direction.

    Raises:
        ValueError: If "direction" is missing or not one of _DIRECTIONS.
    """
    direction = spec.get("direction")
    if direction is None:
        raise ValueError(
            "missing required 'direction' (one of "
            f"{', '.join(sorted(_DIRECTIONS))}); a separation test does not "
            "say which side is allowed to be slower"
        )
    if direction not in _DIRECTIONS:
        raise ValueError(
            f"unknown direction {direction!r} (expected one of "
            f"{', '.join(sorted(_DIRECTIONS))})"
        )
    return str(direction)


def bootstrap_median_diff_ci(
    a: BenchStats,
    b: BenchStats,
    n: int = 10000,
    seed: int = 0,
    confidence: float = 0.95,
) -> tuple[float, float]:
    """Bootstrap a confidence interval for median(b) - median(a).

    Args:
        a: Baseline benchmark stats.
        b: Comparison benchmark stats.
        n: Number of bootstrap resamples.
        seed: RNG seed, for reproducibility.
        confidence: Two-sided confidence level.

    Returns:
        (lower, upper) bound in milliseconds of the resampled median
        difference median(b) - median(a).
    """
    rng = random.Random(seed)
    xs_a, xs_b = a.times_ms, b.times_ms
    diffs = [
        statistics.median(rng.choices(xs_b, k=len(xs_b)))
        - statistics.median(rng.choices(xs_a, k=len(xs_a)))
        for _ in range(n)
    ]
    diffs.sort()
    alpha = (1.0 - confidence) / 2.0
    return _quantile(diffs, alpha), _quantile(diffs, 1.0 - alpha)


def _resolve_path(value: str, base_dir: Path) -> Path:
    p = Path(value)
    return p if p.is_absolute() else base_dir / p


def _resolve_scalar(spec: object, base_dir: Path) -> tuple[float, str]:
    """Resolve an 'abs'/'ratio' value spec.

    Args:
        spec: Either a literal JSON number, or a {"file": ..., "field": ...}
            reference into a hyperfine export (field is a BenchStats
            attribute name, e.g. "median_ms").
        base_dir: Directory relative "file" paths are resolved against.

    Returns:
        (value, description) where description is used in PASS/FAIL output.
    """
    if isinstance(spec, (int, float)) and not isinstance(spec, bool):
        return float(spec), str(spec)
    if isinstance(spec, dict):
        file_ref = spec["file"]
        field = spec["field"]
        stats = load_hyperfine_json(_resolve_path(file_ref, base_dir))
        if not hasattr(stats, field):
            raise ValueError(f"unknown BenchStats field {field!r}")
        return float(getattr(stats, field)), f"{file_ref}:{field}"
    raise TypeError(f"unsupported value spec: {spec!r}")


def _resolve_stats(spec: object, base_dir: Path) -> BenchStats:
    """Resolve an 'iqr'/'bootstrap' file spec (a path string or {"file": ...})."""
    if isinstance(spec, str):
        return load_hyperfine_json(_resolve_path(spec, base_dir))
    if isinstance(spec, dict):
        return load_hyperfine_json(_resolve_path(spec["file"], base_dir))
    raise TypeError(f"unsupported file spec: {spec!r}")


def _require_number(spec: dict[str, object], key: str) -> float:
    """Read a required numeric field from a parsed-JSON dict.

    Raises:
        KeyError: If key is absent.
        TypeError: If the value is not a JSON number.
    """
    if key not in spec:
        raise KeyError(key)
    value = spec[key]
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise TypeError(f"{key!r} must be a number, got {value!r}")
    return float(value)


def _number_or(spec: dict[str, object], key: str, default: float) -> float:
    """Read an optional numeric field from a parsed-JSON dict, or default."""
    return default if key not in spec else _require_number(spec, key)


def evaluate_criterion(spec: dict[str, object], base_dir: Path) -> tuple[bool, str]:
    """Evaluate one thresholds-file criterion.

    Args:
        spec: One value from the thresholds JSON's top-level mapping; must
            have a "kind" of "ratio", "abs", "iqr", or "bootstrap".
        base_dir: Directory relative "file" paths are resolved against.

    Returns:
        (passed, detail) where detail is a human-readable numeric summary.

    Raises:
        ValueError: If "kind" is missing or unrecognized.
    """
    kind = spec.get("kind")

    if kind == "abs":
        lhs_val, lhs_desc = _resolve_scalar(spec["lhs"], base_dir)
        rhs_val, rhs_desc = _resolve_scalar(spec["rhs"], base_dir)
        limit = _require_number(spec, "limit")
        bound = rhs_val + limit
        ok = lhs_val <= bound
        return (
            ok,
            f"{lhs_desc}={lhs_val:.4f} <= {rhs_desc}={rhs_val:.4f} + {limit:.4f} ({bound:.4f})",
        )

    if kind == "ratio":
        lhs_val, lhs_desc = _resolve_scalar(spec["lhs"], base_dir)
        rhs_val, rhs_desc = _resolve_scalar(spec["rhs"], base_dir)
        limit = _require_number(spec, "limit")
        bound = limit * rhs_val
        ok = lhs_val <= bound
        return (
            ok,
            f"{lhs_desc}={lhs_val:.4f} <= {limit:.4f} * {rhs_desc}={rhs_val:.4f} ({bound:.4f})",
        )

    if kind == "iqr":
        direction = _require_direction(spec)
        lhs_stats = _resolve_stats(spec["lhs"], base_dir)
        rhs_stats = _resolve_stats(spec["rhs"], base_dir)
        disjoint = iqr_disjoint(lhs_stats, rhs_stats)
        lhs_below = lhs_stats.q3_ms < rhs_stats.q1_ms
        if direction == "lhs_faster":
            ok = lhs_below
        elif direction == "rhs_faster":
            ok = rhs_stats.q3_ms < lhs_stats.q1_ms
        else:
            ok = disjoint
        side = (
            "lhs below rhs"
            if lhs_below
            else ("rhs below lhs" if disjoint else "overlapping")
        )
        return ok, (
            f"lhs IQR=[{lhs_stats.q1_ms:.4f}, {lhs_stats.q3_ms:.4f}] "
            f"rhs IQR=[{rhs_stats.q1_ms:.4f}, {rhs_stats.q3_ms:.4f}] "
            f"disjoint={disjoint} ({side}) direction={direction}"
        )

    if kind == "bootstrap":
        direction = _require_direction(spec)
        lhs_stats = _resolve_stats(spec["lhs"], base_dir)
        rhs_stats = _resolve_stats(spec["rhs"], base_dir)
        n = int(_number_or(spec, "n", 10000.0))
        seed = int(_number_or(spec, "seed", 0.0))
        confidence = _number_or(spec, "confidence", 0.95)
        lo, hi = bootstrap_median_diff_ci(
            rhs_stats, lhs_stats, n=n, seed=seed, confidence=confidence
        )
        # The CI is on median(lhs) - median(rhs), so an interval entirely
        # below zero means lhs is the faster side and one entirely above
        # zero is a regression of lhs against rhs.
        if direction == "lhs_faster":
            ok = hi < 0.0
        elif direction == "rhs_faster":
            ok = lo > 0.0
        else:
            ok = not (lo <= 0.0 <= hi)
        sign = (
            "lhs faster" if hi < 0.0 else ("lhs slower" if lo > 0.0 else "inconclusive")
        )
        return (
            ok,
            (
                f"{confidence:.0%} CI of median(lhs)-median(rhs) = "
                f"[{lo:.4f}, {hi:.4f}] excludes 0={not (lo <= 0.0 <= hi)} "
                f"({sign}) direction={direction}"
            ),
        )

    raise ValueError(f"unknown criterion kind {kind!r}")


def cmd_compare(args: argparse.Namespace) -> int:
    """Run the `compare` subcommand: evaluate every criterion, print PASS/FAIL.

    Returns:
        0 if every criterion passed, 1 if any failed, 2 if the thresholds file
        itself is unusable (not an object, or empty) or a --require'd
        criterion is absent from it.
    """
    thresholds_path = Path(args.thresholds)
    thresholds: object = json.loads(thresholds_path.read_text())
    base_dir = Path(args.base_dir) if args.base_dir else thresholds_path.parent

    # A thresholds file with no criteria used to exit 0: the loop below ran
    # zero times and `all_ok` stayed True, so "every criterion passed" was
    # reported for a gate that checked nothing. Emptying or truncating the
    # file was therefore the cheapest way to make a benchmark gate green.
    if not isinstance(thresholds, dict):
        print(
            f"{thresholds_path}: thresholds file must be a JSON object of "
            f"criteria, got {type(thresholds).__name__}",
            file=sys.stderr,
        )
        return 2
    if not thresholds:
        print(
            f"{thresholds_path}: thresholds file holds no criteria; refusing "
            "to report a pass for a gate that checked nothing",
            file=sys.stderr,
        )
        return 2

    # Renaming or deleting a criterion is the same hole one level down: the
    # gate stays green because the criterion it was supposed to enforce is
    # simply no longer named. --require lets the caller pin the names it
    # expects to see evaluated.
    missing = [name for name in (args.require or []) if name not in thresholds]
    if missing:
        print(
            f"{thresholds_path}: required criteria absent: {', '.join(missing)}",
            file=sys.stderr,
        )
        return 2

    all_ok = True
    for name, spec in thresholds.items():
        try:
            ok, detail = evaluate_criterion(spec, base_dir)
        except (OSError, ValueError, KeyError, TypeError) as exc:
            ok, detail = False, f"ERROR: {exc}"
        all_ok = all_ok and ok
        print(f"[{'PASS' if ok else 'FAIL'}] {name}: {detail}")

    return 0 if all_ok else 1


def build_parser() -> argparse.ArgumentParser:
    """Build the CLI parser."""
    parser = argparse.ArgumentParser(
        prog="bench-compare.py",
        description=__doc__,
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    compare_parser = subparsers.add_parser(
        "compare", help="Evaluate a thresholds JSON file against hyperfine exports."
    )
    compare_parser.add_argument("thresholds", help="Path to a thresholds JSON file.")
    compare_parser.add_argument(
        "--base-dir",
        default=None,
        help="Base directory for relative 'file' references (default: thresholds file's directory).",
    )
    compare_parser.add_argument(
        "--require",
        action="append",
        metavar="NAME",
        help=(
            "Criterion name that must be present in the thresholds file; "
            "repeatable. Exits 2 if any is absent, so renaming or dropping a "
            "criterion cannot quietly shrink the gate."
        ),
    )
    compare_parser.set_defaults(func=cmd_compare)

    return parser


def main(argv: list[str] | None = None) -> int:
    """CLI entry point."""
    parser = build_parser()
    args = parser.parse_args(argv)
    return int(args.func(args))


if __name__ == "__main__":
    sys.exit(main())
