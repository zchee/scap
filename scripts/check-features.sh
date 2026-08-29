#!/usr/bin/env bash
# V-2 gate: nightly feature gates are bounded by Decision F.
#
# Decision F (docs/plans/2026-08-28-theoretical-limit-optimization.md) permits
# `#![feature(...)]` only inside src/walk/ (the B1 walker path), and only when
# the gate has paid for itself with a measured >= 2 % user-CPU delta on corpus
# a+b. This script enforces both halves:
#
#   1. Location  -- any feature gate outside src/walk/ is an offender.
#   2. Provenance -- the line immediately above the gate must carry a
#      justification comment of the exact form
#
#          // FEATURE(<name>): <reason>, measured <delta> on a+b (Decision F)
#
# Exits 0 when every gate satisfies both rules (and trivially when there are
# none, which is the state today), 1 otherwise, listing each offender as
# `path:line: reason`.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# `#![feature(` (inner, crate/module level) and `#[feature(` (outer). The
# pattern is deliberately literal so a gate cannot hide behind whitespace
# between `#` and `[`.
PATTERN='#!?\[feature\('
ALLOWED_PREFIX='src/walk/'

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

    if [[ "$file" != "$ALLOWED_PREFIX"* ]]; then
        offenders+=("$file:$line: feature gate outside $ALLOWED_PREFIX (Decision F permits it only on the B1 walker path)")
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
echo "check-features.sh: $count feature gate(s), all under $ALLOWED_PREFIX and justified (Decision F satisfied)"
