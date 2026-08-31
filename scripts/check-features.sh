#!/usr/bin/env bash
# V-2 gate: nightly feature gates are bounded by Decision F.
#
# Decision F (docs/plans/2026-08-28-theoretical-limit-optimization.md) permits
# `#![feature(likely_unlikely)]` and `#![feature(cold_path)]` for the B1
# walker path, and only when the gate has paid for itself with a measured
# >= 2 % user-CPU delta on corpus a+b.
#
# Where the gate may be WRITTEN is not where the feature is USED. `#![...]` is
# an inner attribute and `feature` is a crate-level one, so rustc accepts it
# only in the crate root: a `#![feature(...)]` inside src/walk/mod.rs is not a
# narrower gate, it is an ignored one ("crate-level attribute should be in the
# root module"). Enforcing Decision F's "only inside src/walk/" literally
# would therefore have rejected every placement that works and accepted only
# placements that do nothing, which is why this script gates on the crate
# roots instead. Decision F's real constraint -- that the feature exists for
# the B1 path and has a measured delta behind it -- is carried by the
# justification comment, which names the feature and its measurement.
#
# Two rules, both enforced below:
#
#   1. Location  -- a feature gate may appear only in a crate root
#      (src/lib.rs, src/main.rs). Anywhere else is an offender, whether
#      because rustc would ignore it or because it is the outer `#[feature(`
#      form, which is not a feature gate at all.
#   2. Provenance -- the line immediately above the gate must carry a
#      justification comment of the exact form
#
#          // FEATURE(<name>): <reason>, measured <delta> on a+b (Decision F)
#
# Exits 0 when every gate satisfies both rules (and trivially when there are
# none, which is the state today), 1 otherwise, listing each offender as
# `path:line: reason`.
#
# Scope, stated so the exit-0 line is not read as more than it is: the scan
# covers `src/` only. `benches/` and `tests/` each compile as their own crate
# with their own roots, and a `#![feature(...)]` in one of those would not be
# seen here at all. That is a deliberate limit -- Decision F is about the
# shipped binary -- and not a claim that the tree is feature-gate-free. If a
# `src/bin/` binary root is ever added, it belongs in ALLOWED_FILES too, or
# the one legal place to write its gate will be reported as an offender.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# `#![feature(` (inner) and `#[feature(` (outer). Rust attributes are token
# sequences, so `# ! [ feature (` is the same attribute to rustc and the
# pattern tolerates whitespace at every token boundary; a literal
# `#!?\[feature\(` -- what this gate used until W5.4 -- let a gate hide behind
# one space. `[[:space:]]` rather than `\s` because the fallback branch below
# uses `grep -E`, whose POSIX ERE has no `\s`.
#
# Known limit: this is a line-oriented scan, so an attribute split across
# lines still evades it. Closing that needs a token-level scan of the crate
# roots, which is more machinery than a two-feature allowance is worth; the
# roots are short enough to read.
PATTERN='#[[:space:]]*!?[[:space:]]*\[[[:space:]]*feature[[:space:]]*\('

# The only files where rustc honours a crate-level `#![feature(...)]`. Rule 1
# above explains why this is a file set and not the src/walk/ prefix Decision
# F names.
ALLOWED_FILES=('src/lib.rs' 'src/main.rs')

# One `path:line:text` record per match, newline separated. `|| true` keeps
# `set -e` from treating "no matches" (exit 1) as a failure.
if command -v rg >/dev/null 2>&1; then
    matches="$(rg --no-heading --line-number --color=never -e "$PATTERN" src -g '*.rs' || true)"
else
    matches="$(grep -rnE "$PATTERN" src --include='*.rs' || true)"
fi

if [[ -z "$matches" ]]; then
    echo "check-features.sh: no nightly feature gates in src/ (Decision F satisfied)"
    exit 0
fi

# The justification comment that must sit on the line directly above a gate.
# `<name>` is the feature, the rest is free prose up to the measured delta.
JUSTIFICATION='^[[:space:]]*//[[:space:]]*FEATURE\([A-Za-z0-9_]+\):[[:space:]]*.+,[[:space:]]*measured[[:space:]]+.+[[:space:]]+on[[:space:]]+a\+b[[:space:]]+\(Decision F\)[[:space:]]*$'

offenders=()
while IFS= read -r record; do
    [[ -z "$record" ]] && continue
    file="${record%%:*}"
    rest="${record#*:}"
    line="${rest%%:*}"

    allowed=0
    for root in "${ALLOWED_FILES[@]}"; do
        if [[ "$file" == "$root" ]]; then
            allowed=1
            break
        fi
    done
    if (( ! allowed )); then
        offenders+=("$file:$line: feature gate outside the crate roots (${ALLOWED_FILES[*]}); rustc ignores a crate-level #![feature] anywhere else, and Decision F admits no other form")
        continue
    fi

    if (( line < 2 )); then
        offenders+=("$file:$line: feature gate on the first line, so it cannot carry the justification comment above it")
        continue
    fi

    above="$(sed -n "$((line - 1))p" "$file")"
    if [[ ! "$above" =~ $JUSTIFICATION ]]; then
        offenders+=("$file:$line: missing the justification comment on the line above; expected // FEATURE(<name>): <reason>, measured <delta> on a+b (Decision F)")
    fi
done <<< "$matches"

if (( ${#offenders[@]} > 0 )); then
    echo "check-features.sh: ${#offenders[@]} feature-gate violation(s) of Decision F:" >&2
    printf '  %s\n' "${offenders[@]}" >&2
    exit 1
fi

count="$(printf '%s\n' "$matches" | grep -c '' )"
echo "check-features.sh: $count feature gate(s), all in ${ALLOWED_FILES[*]} and justified (Decision F satisfied)"
