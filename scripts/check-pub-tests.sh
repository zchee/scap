#!/usr/bin/env bash
# V-2 / AC-8 gate: every public function is named by at least one test.
#
# Collects each `pub fn` / `pub(crate) fn` declared under src/ -- skipping
# `main` (the binary entry point, exercised end to end by tests/e2e_help.rs)
# and skipping the sibling `*_tests.rs` files themselves -- and requires the
# function's identifier to appear as a whole word somewhere under
# src/**/*_tests.rs or tests/**/*.rs.
#
# This is a name-reachability check, not a coverage measurement: it catches a
# public function that no test so much as mentions, which is the failure the
# plan's §5 "Unit" bullet is guarding against. Judging whether the test that
# names a function actually exercises it stays a human job at review time.
#
# Exits 0 when every name is covered, 1 otherwise, listing the uncovered ones.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

if command -v rg >/dev/null 2>&1; then
    search() { rg --no-heading --line-number --color=never "$@"; }
    declarations="$(rg --no-heading --line-number --color=never \
        -e '^[[:space:]]*pub(\([a-z]+\))? fn [A-Za-z_][A-Za-z0-9_]*' \
        src -g '*.rs' -g '!*_tests.rs' || true)"
else
    search() { grep -rnE "$@"; }
    declarations="$(grep -rnE '^[[:space:]]*pub(\([a-z]+\))? fn [A-Za-z_][A-Za-z0-9_]*' \
        src --include='*.rs' | grep -v '_tests\.rs:' || true)"
fi

# Files a name may be covered by: sibling unit tests and integration tests.
test_files=()
while IFS= read -r f; do
    [[ -n "$f" ]] && test_files+=("$f")
done < <(
    { find src -name '*_tests.rs' -type f
      find tests -name '*.rs' -type f
    } 2>/dev/null | sort
)

if (( ${#test_files[@]} == 0 )); then
    echo "check-pub-tests.sh: no test files found under src/ or tests/" >&2
    exit 1
fi

# name -> the declaration sites that introduced it (several modules define
# `run`, so report every site of an uncovered name).
declared_names=()
declare -A sites=()
while IFS= read -r record; do
    [[ -z "$record" ]] && continue
    file="${record%%:*}"
    rest="${record#*:}"
    line="${rest%%:*}"
    text="${rest#*:}"

    name="$(printf '%s\n' "$text" | sed -E 's/^[[:space:]]*pub(\([a-z]+\))? fn ([A-Za-z_][A-Za-z0-9_]*).*/\2/')"
    [[ -z "$name" || "$name" == "main" ]] && continue

    if [[ -z "${sites[$name]+set}" ]]; then
        declared_names+=("$name")
        sites[$name]="$file:$line"
    else
        sites[$name]="${sites[$name]}, $file:$line"
    fi
done <<< "$declarations"

uncovered=()
for name in "${declared_names[@]}"; do
    if ! search -qw -e "$name" "${test_files[@]}" >/dev/null 2>&1; then
        uncovered+=("$name (declared at ${sites[$name]})")
    fi
done

if (( ${#uncovered[@]} > 0 )); then
    echo "check-pub-tests.sh: ${#uncovered[@]} of ${#declared_names[@]} public function(s) are named by no test:" >&2
    printf '  %s\n' "${uncovered[@]}" >&2
    echo "Add a test under src/**/*_tests.rs or tests/**/*.rs that exercises each." >&2
    exit 1
fi

echo "check-pub-tests.sh: all ${#declared_names[@]} public function(s) in src/ are named by a test"
