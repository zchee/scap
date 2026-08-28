#!/usr/bin/env bash
set -euo pipefail

# scripts/parity-corpus.sh — corpus-level stdout parity between scap and the
# real ghq binary, the design-parity oracle for this project (CLAUDE.md
# "Design parity with ghq"; plan
# .omc/plans/2026-08-28-theoretical-limit-optimization.md, W0.4 / V-4).
#
# Anti-vacuous rule: GHQ_BINARY MUST be set to a real, executable ghq. This
# script never falls back to `command -v ghq` on PATH, so a missing oracle
# fails loudly (exit 2) instead of the check silently passing because no
# oracle ran.
#
# Usage:
#   GHQ_BINARY=/path/to/ghq scripts/parity-corpus.sh <root[:root...]>
#
# Env:
#   GHQ_BINARY  required; path to the oracle ghq binary.
#   SCAP_BIN    optional; default <repo>/target/release/scap.
#
# Compares sorted stdout for: list, list -p, list --unique, list -e ghq,
# list zchee. Prints PASS/FAIL per subcommand and exits non-zero if any
# subcommand's output diverges.

usage() {
  cat >&2 <<'EOF'
Usage: GHQ_BINARY=/path/to/ghq scripts/parity-corpus.sh <root[:root...]>
EOF
}

if [[ $# -ne 1 ]]; then
  usage
  exit 2
fi

roots=$1

if [[ -z "${GHQ_BINARY:-}" ]]; then
  echo "parity-corpus: GHQ_BINARY is unset; refusing to fall back to PATH" >&2
  usage
  exit 2
fi
if [[ ! -x "$GHQ_BINARY" ]]; then
  echo "parity-corpus: GHQ_BINARY=$GHQ_BINARY is not an executable file" >&2
  exit 2
fi

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
scap_bin=${SCAP_BIN:-"$repo_root/target/release/scap"}
if [[ ! -x "$scap_bin" ]]; then
  echo "parity-corpus: SCAP_BIN=$scap_bin is not an executable file (build with: cargo build --release)" >&2
  exit 2
fi

# label:comma,separated,extra,args (beyond the leading "list" subcommand).
specs=(
  "list:"
  "list -p:-p"
  "list --unique:--unique"
  "list -e ghq:-e,ghq"
  "list zchee:zchee"
)

status=0
for spec in "${specs[@]}"; do
  label=${spec%%:*}
  argstr=${spec#*:}
  args=()
  if [[ -n "$argstr" ]]; then
    IFS=',' read -r -a args <<<"$argstr"
  fi

  # A query with no matches is expected to exit 0 with empty stdout for both
  # tools, but this isn't load-bearing for the comparison itself, so errexit
  # is relaxed around the two invocations and every case still reports a
  # PASS/FAIL line rather than aborting the whole corpus run.
  set +e
  ghq_out=$(GHQ_ROOT="$roots" "$GHQ_BINARY" list "${args[@]}" 2>/dev/null | sort)
  scap_out=$(SCAP_ROOT="$roots" "$scap_bin" list "${args[@]}" 2>/dev/null | sort)
  set -e

  if diff_text=$(diff -u <(printf '%s\n' "$ghq_out") <(printf '%s\n' "$scap_out")); then
    printf 'PASS  %s\n' "$label"
  else
    printf 'FAIL  %s\n' "$label"
    printf '%s\n' "$diff_text"
    status=1
  fi
done

exit "$status"
