#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Mechanical checks a benchmark document must pass before it is published.

Three guards, run against a Markdown record and the run directories it cites:

    recompute  every number in the prose is reproduced from the committed
               hyperfine JSON, and the computation that reproduces it is named
    census     every row of every Markdown table has its header's column count
    literals   every backticked code literal is found in the working tree

`recompute` is the pass plan §6 W5.1 item (d) requires. The other two come
from the Phase-4b gate lane's handoff (ledger #25 CLOSED): that lane recorded
that recomputing from JSON catches numeric errors *only*, and that the two
defects its critic actually caught were of different kinds -- a table whose
cell count did not match its header, and a claim whose words did not match the
source it described. One guard per defect class, so a class that has bitten
once cannot bite silently again.

All three are REPORTS, not oracles, and they are meant to be read rather than
merely exited on. `literals` cannot know whether a backticked word is a code
literal or ordinary prose set in code font, and `recompute` cannot know which
of several computations a figure was *meant* to be; each therefore names what
it checked, what it could not classify, and what a human must rule on. A guard
that silently passed what it did not understand would be the failure mode this
project has already paid for.

Exit status is 0 unless --strict is given, in which case any finding exits 1.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

# ---------------------------------------------------------------------------
# Shared Markdown scanning
# ---------------------------------------------------------------------------

_CODE_SPAN = re.compile(r"`[^`]*`")
_DELIM = re.compile(r"^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$")


def iter_prose(path: Path):
    """Yield (line_no, text) for every line outside a fenced code block."""
    in_fence = False
    for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if line.lstrip().startswith("```"):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        yield n, line


# ---------------------------------------------------------------------------
# Guard: table-column census
# ---------------------------------------------------------------------------


def split_cells(line: str) -> list[str]:
    """Split one Markdown table row into cells, honouring escaped pipes.

    A pipe inside an inline-code span is NOT exempt: it really does split the
    row when Markdown renders it, which is precisely the defect the Phase-4b
    critic caught in the plan's D-12 row, where a predicate containing raw
    `|` characters rendered as seven columns in a six-column table. Only a
    backslash-escaped pipe is not a boundary.
    """
    out: list[str] = []
    buf: list[str] = []
    i = 0
    while i < len(line):
        c = line[i]
        if c == "\\" and i + 1 < len(line) and line[i + 1] == "|":
            buf.append("\\|")
            i += 2
            continue
        if c == "|":
            out.append("".join(buf))
            buf = []
            i += 1
            continue
        buf.append(c)
        i += 1
    out.append("".join(buf))
    # Rows are written with leading and trailing pipes, so the split yields an
    # empty string at each end; drop exactly those two.
    if out and not out[0].strip():
        out = out[1:]
    if out and not out[-1].strip():
        out = out[:-1]
    return out


@dataclass
class TableProblem:
    line_no: int
    expected: int
    found: int
    text: str


def census(path: Path) -> tuple[int, int, list[TableProblem]]:
    """Return (tables, rows, problems) over every Markdown table in `path`."""
    lines = path.read_text(encoding="utf-8").splitlines()
    problems: list[TableProblem] = []
    tables = 0
    rows = 0
    in_fence = False
    i = 0
    while i < len(lines):
        line = lines[i]
        if line.lstrip().startswith("```"):
            in_fence = not in_fence
            i += 1
            continue
        if in_fence or "|" not in line:
            i += 1
            continue
        if i + 1 < len(lines) and _DELIM.match(lines[i + 1]) and "|" in lines[i + 1]:
            width = len(split_cells(line))
            tables += 1
            rows += 1
            delim_width = len(split_cells(lines[i + 1]))
            if delim_width != width:
                problems.append(
                    TableProblem(i + 2, width, delim_width, lines[i + 1].strip())
                )
            j = i + 2
            while j < len(lines) and "|" in lines[j] and lines[j].strip():
                found = len(split_cells(lines[j]))
                rows += 1
                if found != width:
                    problems.append(TableProblem(j + 1, width, found, lines[j].strip()))
                j += 1
            i = j
            continue
        i += 1
    return tables, rows, problems


# ---------------------------------------------------------------------------
# Guard: quoted-literal verification
# ---------------------------------------------------------------------------

_SKIP_EXACT = {"-N", "-p", "-e", "--"}


@dataclass
class Literal:
    token: str
    kind: str
    lines: list[int] = field(default_factory=list)
    hits: list[str] = field(default_factory=list)


def classify(tok: str) -> str | None:
    """Name the kind of code literal `tok` is, or None to leave it unchecked.

    A trailing ellipsis is stripped first. A record abbreviates a digest as
    `074bbdd42b50dfe0…`, and without this every such token fell through every
    rule and was silently left unchecked - the guard reporting "0 not found"
    while never having looked at the fingerprints that matter most.
    """
    if not tok or tok in _SKIP_EXACT:
        return None
    t = tok.strip().rstrip("….")
    if not t:
        return None
    if " " in t and not t.startswith("-"):
        return None
    if t.startswith("--") or (t.startswith("-") and len(t) <= 3 and t[1:].isalpha()):
        return "flag"
    if re.fullmatch(r"[A-Z][A-Z0-9_]{2,}", t):
        return "env"
    # Before the identifier rule: an abbreviated commit is all hex and would
    # otherwise be searched for as a function name and reported missing --
    # the kind of noise that teaches a reader to skip a guard's output.
    # Up to 64, not 40: a full sha256 written out is a digest, and capping at
    # a git object's length let one fall through to the identifier rule and be
    # searched for as a function name.
    if re.fullmatch(r"[0-9a-f]{7,64}", t):
        return "commit"
    if re.fullmatch(
        r"[\w./-]+\.(rs|sh|py|toml|md|json|lock|bin|dump)(:\d+(-\d+)?)?", t
    ):
        return "path"
    if re.fullmatch(r"[a-z][a-zA-Z0-9]*\.[a-z][a-zA-Z0-9]*", t):
        return "config-key"
    if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*::[A-Za-z0-9_:]+", t):
        return "path-expr"
    if re.fullmatch(r"[a-z_][a-z0-9_]{2,}(\(\))?", t):
        return "identifier"
    return None


def collect_literals(path: Path) -> dict[str, Literal]:
    lits: dict[str, Literal] = {}
    for n, line in iter_prose(path):
        for m in _CODE_SPAN.finditer(line):
            tok = m.group(0)[1:-1]
            kind = classify(tok)
            if kind is None:
                continue
            lits.setdefault(tok, Literal(tok, kind)).lines.append(n)
    return lits


_SEARCH_ROOTS = ["src", "scripts", "tests", "docs/benchmarks", "Cargo.toml"]


def _grep(repo: Path, needle: str) -> list[str]:
    """Files under the searched roots containing `needle` as a fixed string.

    docs/benchmarks is searched alongside the source tree because a benchmark
    record legitimately quotes the row names defined in its own row file;
    omitting it reported every row name as missing.
    """
    cmd = [
        "grep",
        "-rl",
        "--include=*.rs",
        "--include=*.sh",
        "--include=*.py",
        "--include=*.toml",
        "-F",
        needle,
        *_SEARCH_ROOTS,
    ]
    res = subprocess.run(cmd, cwd=repo, capture_output=True, text=True, check=False)
    return [f for f in res.stdout.split("\n") if f]


_MANIFEST = Path(".omc/bench/binaries/SHA256SUMS")


def _manifest_digests(repo: Path) -> list[str]:
    """The sha256 digests the reference-binary manifest records.

    The manifest is git-ignored, so a document pins its binaries by quoting
    digests from it. Reading it here is what lets an abbreviated fingerprint
    be verified rather than merely reported as unknown.
    """
    mf = repo / _MANIFEST
    if not mf.exists():
        return []
    out = []
    for line in mf.read_text(encoding="utf-8").splitlines():
        parts = line.split()
        if parts and len(parts[0]) == 64:
            out.append(parts[0])
    return out


def _tracked_by_name(repo: Path, name: str) -> list[str]:
    """Tracked files whose basename is `name`.

    git's own index is the source: it is exact, it is fast, and it excludes
    build output, so a hit means a file this repository really ships rather
    than something left in target/.
    """
    res = subprocess.run(
        ["git", "ls-files"], cwd=repo, capture_output=True, text=True, check=False
    )
    return [f for f in res.stdout.split("\n") if f and Path(f).name == name]


_SHA256 = re.compile(r"\b[0-9a-f]{64}\b")


def verify_literals(
    lits: dict[str, Literal], repo: Path, doc: Path | None = None
) -> None:
    """Check each literal against the tree, recording where it was found.

    Full digests written out in the document itself count as known digests
    alongside the manifest's: a record pins a binary that is deliberately not
    committed by quoting its sha256, and an abbreviation of a digest the same
    document spells in full is verified, not unknown.
    """
    _doc_digests: list[str] = []
    if doc is not None and doc.exists():
        _doc_digests = _SHA256.findall(doc.read_text(encoding="utf-8"))
    for lit in lits.values():
        # The same normalisation classify() applied: the checks below compare
        # the literal against real names and digests, and a trailing ellipsis
        # belongs to the prose, not to the value.
        tok = lit.token.strip().rstrip("….")

        if lit.kind == "path":
            bare = tok.split(":")[0]
            if (repo / bare).exists():
                lit.hits.append(f"file exists: {bare}")
                continue
            # A record often names a file by its basename alone -
            # `metadata.json`, `summary.md` - because it appears once per run
            # directory rather than at one path. Reporting those as missing
            # would be a false alarm, so a tracked file of that name counts.
            hits = _tracked_by_name(repo, Path(bare).name)
            lit.hits = hits[:3]
            continue

        if lit.kind == "commit":
            res = subprocess.run(
                ["git", "cat-file", "-t", tok],
                cwd=repo,
                capture_output=True,
                text=True,
                check=False,
            )
            if res.returncode == 0:
                lit.hits.append(f"git object: {res.stdout.strip()}")
                continue
            # Hex that git does not know may still be a BINARY FINGERPRINT
            # abbreviated to its first bytes. Those are checkable: the
            # reference binaries carry a SHA256SUMS manifest, so a token that
            # prefixes exactly one digest in it is verified as that binary -
            # and, more usefully, a MISTYPED prefix now fails instead of
            # passing as prose.
            # The document is a source ONLY for abbreviations. A full-length
            # digest matched against the document that contains it verifies
            # against itself, which no fabricated value can fail - the check
            # would be vacuous. A full digest must match the manifest or be
            # reported as pinned-but-unverifiable; an abbreviation may match a
            # full form spelled out in the same document, which catches a
            # mistyped prefix and is honestly labelled as internal
            # consistency rather than verification.
            known = _manifest_digests(repo)
            matches = [d for d in known if d.startswith(tok)]
            source = "manifest"
            if not matches and len(tok) < 64:
                matches = [d for d in _doc_digests if d.startswith(tok)]
                source = "full sha256 in this document (internal consistency)"
            if matches:
                lit.kind = "digest"
                lit.hits.append(
                    f"binary fingerprint via {source}: {matches[0][:24]}..."
                )
                continue
            if len(tok) >= 16:
                # Hex this long that neither git nor the manifest knows is a
                # content digest the tree cannot confirm: a stdout sha256, or
                # a fingerprint of an artifact nobody preserved. Relabel it so
                # it is not reported as a missing commit -- but do NOT mark it
                # verified. A branch that passed every long hex string would
                # pass a fabricated one too, and making a mistyped digest fail
                # is the whole point of this guard. It is reported instead,
                # for a human to rule on.
                lit.kind = "digest"
            continue

        if lit.kind == "flag":
            # clap's derive API spells `--no-cache` as the field `no_cache`,
            # so the dashed form need not appear in the source at all. Either
            # spelling counts; only a flag matching neither is a finding.
            lit.hits = _grep(repo, tok) or _grep(
                repo, tok.lstrip("-").replace("-", "_")
            )
            continue

        if lit.kind == "path-expr":
            # `Cache::path_for` is written `fn path_for` at its definition.
            lit.hits = _grep(repo, tok) or _grep(repo, tok.rsplit("::", 1)[-1])
            continue

        lit.hits = _grep(repo, tok.removesuffix("()"))


# ---------------------------------------------------------------------------
# Guard: recompute every figure from the committed run JSON
# ---------------------------------------------------------------------------


def _quantile(sorted_vals: list[float], p: float) -> float:
    n = len(sorted_vals)
    if n == 1:
        return sorted_vals[0]
    h = p * (n - 1)
    lo, hi = int(h // 1), min(int(-(-h // 1)), n - 1)
    if lo == hi:
        return sorted_vals[lo]
    return sorted_vals[lo] + (h - lo) * (sorted_vals[hi] - sorted_vals[lo])


@dataclass
class Row:
    group: str
    name: str
    stats: dict[str, float]


def load_rows(run_dirs: list[Path]) -> list[Row]:
    """Read every per-row hyperfine export under the given run directories.

    hyperfine reports per-run wall times but only mean user and system times,
    so every wall statistic here is a true order statistic while user, system
    and their sum are means -- the same asymmetry bench-analyze.py documents.
    """
    rows: list[Row] = []
    for d in run_dirs:
        for jf in sorted(d.glob("*.json")):
            if jf.name == "metadata.json":
                continue
            try:
                data = json.loads(jf.read_text(encoding="utf-8"))
                r = data["results"][0]
            except (KeyError, IndexError, json.JSONDecodeError):
                continue
            times = sorted(t * 1000.0 for t in r["times"])
            user = r["user"] * 1000.0
            system = r["system"] * 1000.0
            rows.append(
                Row(
                    d.name,
                    jf.stem,
                    {
                        "mean": r["mean"] * 1000.0,
                        "stddev": r["stddev"] * 1000.0,
                        "median": r["median"] * 1000.0,
                        "min": r["min"] * 1000.0,
                        "max": r["max"] * 1000.0,
                        "q1": _quantile(times, 0.25),
                        "q3": _quantile(times, 0.75),
                        "user": user,
                        "sys": system,
                        "cpu": user + system,
                    },
                )
            )
    return rows


def _flatten_numbers(node: object, prefix: str, vals: dict[str, float]) -> None:
    """Record every number reachable in `node` under its dotted path.

    Defined at module scope rather than nested in the loop below: a closure
    over the caller's accumulator is what ruff's B023 warns about, and the
    warning is right - the binding would be the loop's, not the iteration's.
    """
    if isinstance(node, dict):
        for k, v in node.items():
            _flatten_numbers(v, f"{prefix}.{k}" if prefix else k, vals)
    elif isinstance(node, list):
        for i, v in enumerate(node):
            _flatten_numbers(v, f"{prefix}[{i}]", vals)
    elif isinstance(node, bool):
        return
    elif isinstance(node, (int, float)):
        vals[prefix] = float(node)
    elif isinstance(node, str):
        # `sysctl -n vm.loadavg` is recorded verbatim as "{ a b c }".
        for i, tok in enumerate(re.findall(r"\d+\.\d+", node)):
            vals[f"{prefix}[{i}]"] = float(tok)


def load_metadata(run_dirs: list[Path]) -> list[tuple[str, dict[str, float]]]:
    """Read the gate and environment figures each group's metadata.json records.

    A benchmark record quotes these as often as it quotes timings - the gate
    readings that admitted a group, the load averages, the binary size, the
    corpus counters - and a guard blind to them reports each one as an
    unreproduced figure. That noise is what stops a guard being read, so the
    metadata is a first-class source here rather than an afterthought.
    """
    out: list[tuple[str, dict[str, float]]] = []
    for d in run_dirs:
        mf = d / "metadata.json"
        if not mf.exists():
            continue
        try:
            m = json.loads(mf.read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            continue
        vals: dict[str, float] = {}
        _flatten_numbers(m, "", vals)
        out.append((d.name, vals))
    return out


_POOL_SUFFIX = re.compile(r"_\d+$")


def add_pooled_rows(rows: list[Row]) -> list[Row]:
    """Add a pseudo-row for each set of repeated readings of one binary.

    The row files name repeated readings of the same binary in one group
    `<base>_1`, `<base>_2`, `<base>_3` precisely so a record can pool them,
    and pooling them is what makes a comparison robust against this host's
    within-group drift. A document therefore quotes the pooled figure and the
    ratios taken from it, so a guard that only knows the individual rows
    cannot check its most important numbers. The naming convention defines
    the pool; nothing is inferred from the values.
    """
    groups: dict[tuple[str, str], list[Row]] = {}
    for r in rows:
        base = _POOL_SUFFIX.sub("", r.name)
        if base != r.name:
            groups.setdefault((r.group, base), []).append(r)
    pooled: list[Row] = []
    for (group, base), members in groups.items():
        if len(members) < 2:
            continue
        stats = {
            k: sum(m.stats[k] for m in members) / len(members) for k in members[0].stats
        }
        pooled.append(Row(group, f"{base}(pooled x{len(members)})", stats))
    return rows + pooled


def derive(rows: list[Row]) -> dict[str, list[str]]:
    """Every value a figure in the document could legitimately be.

    Raw statistics, then within-group pairwise ratios, percentage forms,
    differences and percentage changes. Each is filed under its own printed
    form at one through four decimal places, together with the computation
    that produced it, so a match can be read as provenance rather than taken
    on trust: a value reproduced by the wrong computation is still a defect,
    and only naming the computation makes that visible.
    """
    table: dict[str, list[str]] = {}

    def put(value: float, how: str) -> None:
        if not math.isfinite(value):
            return
        for dp in range(5):
            table.setdefault(f"{value:.{dp}f}", []).append(how)
            if abs(value) >= 1000:
                table.setdefault(f"{value:,.{dp}f}", []).append(how)

    for r in rows:
        for stat, v in r.stats.items():
            put(v, f"{r.group}/{r.name}.{stat}")

    by_group: dict[str, list[Row]] = {}
    for r in rows:
        by_group.setdefault(r.group, []).append(r)

    for group, grp in by_group.items():
        for a in grp:
            for b in grp:
                if a.name >= b.name:
                    continue
                for stat in ("median", "mean", "user", "sys", "cpu"):
                    av, bv = a.stats[stat], b.stats[stat]
                    if bv:
                        put(av / bv, f"{group}: {a.name}.{stat} / {b.name}.{stat}")
                        put(
                            av / bv * 100.0,
                            f"{group}: {a.name}.{stat} / {b.name}.{stat} as %",
                        )
                        put(
                            (av / bv - 1.0) * 100.0,
                            f"{group}: {a.name}.{stat} vs {b.name}.{stat} % change",
                        )
                    if av:
                        put(bv / av, f"{group}: {b.name}.{stat} / {a.name}.{stat}")
                        put(
                            (bv / av - 1.0) * 100.0,
                            f"{group}: {b.name}.{stat} vs {a.name}.{stat} % change",
                        )
                    put(av - bv, f"{group}: {a.name}.{stat} - {b.name}.{stat}")
                    put(bv - av, f"{group}: {b.name}.{stat} - {a.name}.{stat}")
                    # A record routinely pools the two readings of one binary
                    # inside a group -- "5.369 / 5.111 -> 5.240" -- so the
                    # pooled value is a figure the document legitimately
                    # carries and the guard must be able to reproduce it.
                    put(
                        (av + bv) / 2.0,
                        f"{group}: mean of {a.name}.{stat} and {b.name}.{stat}",
                    )
    return table


def derive_metadata(
    meta: list[tuple[str, dict[str, float]]], table: dict[str, list[str]]
) -> None:
    """Fold every metadata figure into the same lookup table."""

    def put(value: float, how: str) -> None:
        if not math.isfinite(value):
            return
        for dp in range(5):
            table.setdefault(f"{value:.{dp}f}", []).append(how)
            if abs(value) >= 1000:
                table.setdefault(f"{value:,.{dp}f}", []).append(how)

    for group, vals in meta:
        for key, v in vals.items():
            put(v, f"{group}/metadata.{key}")


# A figure is a decimal number, optionally with thousands separators. Dates,
# ISO timestamps, run-directory names, section numbers and file:line
# references are excluded by the guards below rather than by the pattern,
# because each needs a different test.
_NUM = re.compile(r"(?<![\w.:-])(\d{1,3}(?:,\d{3})+|\d+)(?:\.(\d+))?(?![\w:-])")
_DATEISH = re.compile(r"\d{4}-\d{2}-\d{2}|\d{8}T\d{6}Z|§\s*\d|AC-\d|D-\d|W\d|ADR-\d")


@dataclass
class Figure:
    text: str
    line_no: int
    context: str
    how: list[str] = field(default_factory=list)


def collect_figures(path: Path, min_decimals: int) -> list[Figure]:
    """Every numeric literal in the prose that a run JSON could reproduce.

    Integers are skipped by default: counts such as 845 repositories or 30
    runs are not derived from timing JSON, and including them would bury the
    timing figures this guard exists to check under matches that mean nothing.
    """
    figures: list[Figure] = []
    for n, line in iter_prose(path):
        if _DATEISH.search(line) and min_decimals == 0:
            continue
        for m in _NUM.finditer(line):
            decimals = len(m.group(2) or "")
            if decimals < min_decimals:
                continue
            span = m.group(0)
            before = line[max(0, m.start() - 30) : m.start()]
            if re.search(r"(\.rs|\.sh|\.py|\.md):$", before):
                continue
            figures.append(Figure(span, n, line.strip()[:150]))
    return figures


def match_figures(figures: list[Figure], table: dict[str, list[str]]) -> None:
    for f in figures:
        f.how = table.get(f.text, []) or table.get(f.text.replace(",", ""), [])


# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------


def cmd_census(args: argparse.Namespace) -> int:
    tables, rows, problems = census(args.doc)
    print(f"census: {args.doc}")
    print(f"  tables: {tables}")
    print(f"  rows (header + body, delimiter rows excluded): {rows}")
    if not problems:
        print("  RESULT: every row matches its header's column count")
        return 0
    print(f"  RESULT: {len(problems)} row(s) do NOT match their header:")
    for p in problems:
        print(f"    line {p.line_no}: expected {p.expected} cells, found {p.found}")
        print(f"      {p.text[:160]}")
    return 1


def cmd_literals(args: argparse.Namespace) -> int:
    lits = collect_literals(args.doc)
    verify_literals(lits, args.repo, args.doc)
    found = {k: v for k, v in lits.items() if v.hits}
    missing = {k: v for k, v in lits.items() if not v.hits}
    print(f"literals: {args.doc}  (tree: {args.repo})")
    print(
        f"  classified: {len(lits)}   verified: {len(found)}   NOT FOUND: {len(missing)}"
    )
    by_kind: dict[str, int] = {}
    for lit in lits.values():
        by_kind[lit.kind] = by_kind.get(lit.kind, 0) + 1
    print("  by kind: " + ", ".join(f"{k}={v}" for k, v in sorted(by_kind.items())))
    if missing:
        print("  NOT FOUND in the tree - each needs a human ruling:")
        for lit in sorted(missing.values(), key=lambda x: x.token):
            where = ",".join(str(n) for n in lit.lines[:6])
            print(f"    [{lit.kind}] {lit.token}   (doc lines {where})")
    if args.show_found:
        print("  verified:")
        for lit in sorted(found.values(), key=lambda x: x.token):
            print(f"    [{lit.kind}] {lit.token} -> {lit.hits[0]}")
    return 1 if missing else 0


def cmd_recompute(args: argparse.Namespace) -> int:
    rows = add_pooled_rows(load_rows(args.runs))
    meta = load_metadata(args.runs)
    table = derive(rows)
    derive_metadata(meta, table)
    figures = collect_figures(args.doc, args.min_decimals)
    match_figures(figures, table)
    matched = [f for f in figures if f.how]
    unmatched = [f for f in figures if not f.how]
    print(f"recompute: {args.doc}")
    print(
        f"  run directories: {len(args.runs)}   rows loaded: {len(rows)}"
        f"   metadata files: {len(meta)}"
    )
    print(f"  figures checked (>= {args.min_decimals} decimal places): {len(figures)}")
    print(
        f"  reproduced from run JSON: {len(matched)}   NOT reproduced: {len(unmatched)}"
    )
    if unmatched:
        print("  NOT reproduced - each needs a human ruling:")
        for f in unmatched:
            print(f"    line {f.line_no}: {f.text}")
            print(f"      {f.context}")
    if args.show_provenance:
        print("  provenance of every reproduced figure:")
        for f in matched:
            print(
                f"    line {f.line_no}: {f.text} <- {f.how[0]}"
                + (
                    f"  (+{len(f.how) - 1} other computations)"
                    if len(f.how) > 1
                    else ""
                )
            )
    return 1 if unmatched else 0


def cmd_all(args: argparse.Namespace) -> int:
    rc = 0
    rc |= cmd_recompute(args)
    print()
    rc |= cmd_census(args)
    print()
    rc |= cmd_literals(args)
    return rc


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    p.add_argument(
        "--strict", action="store_true", help="exit 1 when any guard reports a finding"
    )
    sub = p.add_subparsers(dest="cmd", required=True)

    def add_doc(sp: argparse.ArgumentParser) -> None:
        sp.add_argument("doc", type=Path)

    cen = sub.add_parser("census", help="table-column census")
    add_doc(cen)
    cen.set_defaults(func=cmd_census)

    lit = sub.add_parser("literals", help="grep-verify backticked literals")
    add_doc(lit)
    lit.add_argument("--repo", type=Path, default=Path.cwd())
    lit.add_argument("--show-found", action="store_true")
    lit.set_defaults(func=cmd_literals)

    rec = sub.add_parser("recompute", help="reproduce every figure from run JSON")
    add_doc(rec)
    rec.add_argument("runs", type=Path, nargs="*", default=[])
    rec.add_argument(
        "--min-decimals",
        type=int,
        default=1,
        help="skip figures with fewer decimal places (default 1: "
        "integers are counts, not timing statistics)",
    )
    rec.add_argument("--show-provenance", action="store_true")
    rec.set_defaults(func=cmd_recompute)

    everything = sub.add_parser("all", help="all three guards")
    add_doc(everything)
    everything.add_argument("runs", type=Path, nargs="*", default=[])
    everything.add_argument("--repo", type=Path, default=Path.cwd())
    everything.add_argument("--min-decimals", type=int, default=1)
    everything.add_argument("--show-found", action="store_true")
    everything.add_argument("--show-provenance", action="store_true")
    everything.set_defaults(func=cmd_all)
    return p


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    rc = args.func(args)
    return rc if args.strict else 0


if __name__ == "__main__":
    sys.exit(main())
