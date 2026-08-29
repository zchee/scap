#!/usr/bin/env bash
# V-7 gate: the configuration a real machine actually has resolves the same
# way through every path ADR-8 offers.
#
# Two comparisons, run against the invoking user's real gitconfig rather than
# a fixture -- fixtures are already covered by tests/config_oracle.rs, and
# what this script exists to catch is the include chain, the system file and
# the `~/` spellings that only a real configuration has:
#
#   1. backend parity  -- `scap root` and `scap root --all` must be
#      byte-identical with the in-process (A4) default and with
#      SCAP_CONFIG_BACKEND=git (A3, git as the parser of record).
#   2. ghq parity      -- `scap root --all` vs `ghq root --all`. V-7 treats
#      the ghq leg as required, so a missing ghq is exit 2 (nothing was
#      compared), never a silent pass. scap reads `scap.root` and ghq reads
#      `ghq.root`; when the invoking user's configuration sets only one of
#      them the two commands are not comparable, and that is reported as a
#      skip of *that leg* while the backend legs still decide the exit code.
#
# Usage:
#   scripts/config-parity.sh
#
# Env:
#   SCAP_BIN    optional; default <repo>/target/release/scap.
#   GHQ_BINARY  optional; default `command -v ghq`.
#
# Exit codes: 0 all comparisons equal; 1 a comparison diverged; 2 an oracle
# is missing, so nothing meaningful was compared (no scap binary, or no ghq).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

SCAP_BIN="${SCAP_BIN:-$REPO_ROOT/target/release/scap}"
GHQ_BINARY="${GHQ_BINARY:-$(command -v ghq || true)}"

if [[ ! -x "$SCAP_BIN" ]]; then
  echo "config-parity: no scap binary at $SCAP_BIN (build it, or set SCAP_BIN)" >&2
  exit 2
fi

if [[ -z "$GHQ_BINARY" || ! -x "$GHQ_BINARY" ]]; then
  echo "config-parity: no executable ghq (set GHQ_BINARY); refusing to pass without the oracle" >&2
  exit 2
fi

status=0

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Compare two command outputs, reporting a unified diff on divergence.
compare() {
  local label=$1 expected_file=$2 actual_file=$3
  if diff -u "$expected_file" "$actual_file" >"$work/diff" 2>&1; then
    echo "PASS  $label"
  else
    echo "FAIL  $label"
    sed 's/^/      /' "$work/diff" >&2
    status=1
  fi
  rm -f "$work/diff"
}

for args in "root" "root --all"; do
  # shellcheck disable=SC2086 # deliberate word splitting: `root --all`.
  "$SCAP_BIN" $args >"$work/default.out" 2>"$work/default.err" || {
    echo "FAIL  scap $args (default backend) exited non-zero" >&2
    sed 's/^/      /' "$work/default.err" >&2
    status=1
    continue
  }
  # shellcheck disable=SC2086
  SCAP_CONFIG_BACKEND=git "$SCAP_BIN" $args >"$work/git.out" 2>"$work/git.err" || {
    echo "FAIL  scap $args (SCAP_CONFIG_BACKEND=git) exited non-zero" >&2
    sed 's/^/      /' "$work/git.err" >&2
    status=1
    continue
  }
  compare "scap $args: default vs SCAP_CONFIG_BACKEND=git" "$work/default.out" "$work/git.out"
done

if ! git config --get scap.root >/dev/null 2>&1; then
  echo "SKIP  scap root --all vs ghq root --all (scap.root is unset in this configuration)"
elif ! git config --get ghq.root >/dev/null 2>&1; then
  echo "SKIP  scap root --all vs ghq root --all (ghq.root is unset in this configuration)"
else
  "$SCAP_BIN" root --all >"$work/scap-all.out"
  "$GHQ_BINARY" root --all >"$work/ghq-all.out"
  compare "scap root --all vs ghq root --all" "$work/ghq-all.out" "$work/scap-all.out"
fi

exit "$status"
