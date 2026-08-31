#!/usr/bin/env bash
set -euo pipefail

# scripts/parity-corpus.sh — corpus-level stdout parity between scap and the
# real ghq binary, the design-parity oracle for this project (CLAUDE.md
# "Design parity with ghq"; plan
# .omc/plans/2026-08-28-theoretical-limit-optimization.md, W0.4 / V-4).
#
# Anti-vacuous rules. A parity gate that can report PASS without either tool
# having produced anything proves nothing, so three conditions must hold
# before any PASS is printed:
#
#   1. GHQ_BINARY MUST be set to a real, executable ghq. This script never
#      falls back to `command -v ghq` on PATH, so a missing oracle fails
#      loudly (exit 2) instead of the check silently passing because no
#      oracle ran.
#   2. Both invocations of every spec MUST exit 0. An exit status is a
#      result, not noise: two crashed tools also produce two empty stdouts,
#      which a plain `diff` reads as agreement. Each spec's stderr is
#      captured and printed on FAIL.
#   3. The unfiltered `list` census spec MUST emit at least
#      SCAP_PARITY_MIN_LINES repositories through the oracle. This is what
#      distinguishes a real corpus from an empty root: without it, running
#      the gate against `mktemp -d` reports PASS on all five specs and
#      exits 0. The census is the first spec, and a census failure aborts
#      the run before any later spec can print PASS.
#
# The filtered specs may legitimately match nothing on a given corpus; the
# census is what makes their emptiness meaningful rather than vacuous.
#
# Usage:
#   GHQ_BINARY=/path/to/ghq scripts/parity-corpus.sh <root[:root...]>
#
# Env:
#   GHQ_BINARY            required; path to the oracle ghq binary.
#   SCAP_BIN              optional; default <repo>/target/release/scap.
#   SCAP_PARITY_MIN_LINES optional; default 100. The floor the unfiltered
#                         `list` census must clear. 100 is well under any
#                         corpus this gate is run against in the plan's V-4
#                         (the recorded runs list 800+ repositories) and well
#                         over anything a stray empty or half-populated root
#                         could produce by accident.
#
# Compares sorted stdout for: list, list -p, list --unique, list -e ghq,
# list zchee. Prints PASS/FAIL per subcommand and exits non-zero if any
# subcommand's output diverges, either tool fails, or the census is short.

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

min_lines=${SCAP_PARITY_MIN_LINES:-100}
if [[ ! "$min_lines" =~ ^[0-9]+$ ]]; then
  echo "parity-corpus: SCAP_PARITY_MIN_LINES=$min_lines is not a non-negative integer" >&2
  exit 2
fi
# A floor of 0 is accepted -- there are legitimate reasons to run this against
# a deliberately small fixture -- but it puts the run back in exactly the state
# anti-vacuous rule 3 exists to prevent, so it says so on stderr. A PASS with
# this line above it and a PASS without it are not the same evidence.
if [[ "$min_lines" -eq 0 ]]; then
  echo "parity-corpus: census floor is disabled; this run does not prove parity" >&2
fi

errdir=$(mktemp -d "${TMPDIR:-/tmp}/scap-parity-corpus.XXXXXX")
trap 'rm -rf "$errdir"' EXIT

# Number of lines in a captured stdout. `printf '%s\n' ""` is one empty line,
# not zero, so an empty capture has to be special-cased or the census floor
# would count a silent tool as having produced one repository.
count_lines() {
  if [[ -z "$1" ]]; then
    printf '0'
  else
    printf '%s\n' "$1" | wc -l | tr -d ' '
  fi
}

# label:comma,separated,extra,args (beyond the leading "list" subcommand).
#
# The first entry is the census spec (see anti-vacuous rule 3): it must be
# the unfiltered `list`, because it is the only spec whose output size is a
# statement about the corpus rather than about a filter.
specs=(
  "list:"
  "list -p:-p"
  "list --unique:--unique"
  "list -e ghq:-e,ghq"
  "list zchee:zchee"
)

# The census rule is enforced positionally, so the position has to be checked
# rather than assumed: reordering this list, or adding a filtered spec at the
# top, would otherwise move the corpus-size test onto a filter's output and
# quietly restore the vacuous PASS.
if [[ "${specs[0]}" != "list:" ]]; then
  echo "parity-corpus: specs[0] must be the unfiltered census spec ('list:'), got '${specs[0]}'" >&2
  exit 2
fi

status=0
census_done=0
for spec in "${specs[@]}"; do
  label=${spec%%:*}
  argstr=${spec#*:}
  args=()
  if [[ -n "$argstr" ]]; then
    IFS=',' read -r -a args <<<"$argstr"
  fi

  # errexit is relaxed around the two invocations so that every spec still
  # reports its own PASS/FAIL line rather than aborting the corpus run; the
  # exit statuses are read back explicitly and are load-bearing. `pipefail`
  # stays on, so a failing tool is not masked by the `sort` that follows it.
  set +e
  ghq_out=$(GHQ_ROOT="$roots" "$GHQ_BINARY" list "${args[@]}" 2>"$errdir/ghq.err" | sort)
  ghq_status=$?
  scap_out=$(SCAP_ROOT="$roots" "$scap_bin" list "${args[@]}" 2>"$errdir/scap.err" | sort)
  scap_status=$?
  set -e

  if [[ $ghq_status -ne 0 || $scap_status -ne 0 ]]; then
    printf 'FAIL  %s (ghq exit %d, scap exit %d)\n' "$label" "$ghq_status" "$scap_status"
    printf '  ghq stderr: %s\n' "$(<"$errdir/ghq.err")"
    printf '  scap stderr: %s\n' "$(<"$errdir/scap.err")"
    status=1
    if [[ $census_done -eq 0 ]]; then
      echo "parity-corpus: census spec failed; refusing to run the remaining specs" >&2
      exit "$status"
    fi
    continue
  fi

  if [[ $census_done -eq 0 ]]; then
    census_done=1
    ghq_lines=$(count_lines "$ghq_out")
    if [[ "$ghq_lines" -lt "$min_lines" ]]; then
      printf 'FAIL  %s (census: ghq listed %s repositories, need >= %s)\n' \
        "$label" "$ghq_lines" "$min_lines"
      printf '  ghq stderr: %s\n' "$(<"$errdir/ghq.err")"
      printf '  scap stderr: %s\n' "$(<"$errdir/scap.err")"
      echo "parity-corpus: corpus too small to prove parity; refusing to run the remaining specs" >&2
      echo "parity-corpus: point <root> at a populated corpus, or lower SCAP_PARITY_MIN_LINES deliberately" >&2
      status=1
      exit "$status"
    fi
  fi

  if diff_text=$(diff -u <(printf '%s\n' "$ghq_out") <(printf '%s\n' "$scap_out")); then
    printf 'PASS  %s\n' "$label"
  else
    printf 'FAIL  %s\n' "$label"
    printf '%s\n' "$diff_text"
    # Both tools exited 0 to reach this branch, so stderr is incidental here
    # and usually empty; print it only when there is something to read. The
    # two branches above print it unconditionally on purpose -- there, a tool
    # that failed while saying nothing is itself the finding.
    if [[ -s "$errdir/ghq.err" ]]; then
      printf '  ghq stderr: %s\n' "$(<"$errdir/ghq.err")"
    fi
    if [[ -s "$errdir/scap.err" ]]; then
      printf '  scap stderr: %s\n' "$(<"$errdir/scap.err")"
    fi
    status=1
  fi
done

exit "$status"
