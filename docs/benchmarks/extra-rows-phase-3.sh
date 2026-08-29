#!/usr/bin/env bash
# Extra row_* functions for scripts/bench-quiet.sh, selected via
# SCAP_BENCH_EXTRA + SCAP_BENCH_ROWS. Sourced by the harness, so ENV_BIN,
# SCAP_BIN, ROOT_A, ROOT_B, ROOT_AB and run_bench all come from it; this file
# is not executable on its own.
#
# These are the W3.0b rows: the `.git`-detection strategy matrix that freezes
# the walker's default (plan section 6, "W3.0b"; deviation D-6), and the
# thread sweep that re-states the Decision-B thread rule on the shipped
# walker. Every row passes its knobs to the measured process with `env` on
# the command line rather than exporting them around the harness, so a
# committed run is reproducible from committed files alone and no row can
# inherit a stray value from the pane that started it.
#
# shellcheck shell=bash

# Corpus a-prime's defining exclusion, identical to extra-rows-w2b1.sh's --
# the harness's inventory section reads $APRIME_EXCLUDE, so any extra-rows
# file that defines an a-prime row must define it too.
APRIME_EXCLUDE="${SCAP_BENCH_APRIME_EXCLUDE:-github.com/zchee/claude-code.bak}"

# `scap list` over one corpus with an explicit detection strategy, and
# optionally an explicit worker-thread count.
#
# `SCAP_LIST_DETECT` is set even for the value that is already the default:
# a row that relied on the default would silently change meaning the moment
# W3.0b freezes a different one, and the two arms of a comparison must be
# spelled the same way for the comparison to mean anything.
row_detect_at() {
  local name="$1" rootval="$2" detect="$3" threads="${4:-}"
  local -a envs=("SCAP_ROOT=$rootval" "SCAP_LIST_DETECT=$detect")
  [[ -n "$threads" ]] && envs+=("SCAP_LIST_THREADS=$threads")
  run_bench "$name" "$ENV_BIN" "${envs[@]}" "$SCAP_BIN" list
}

# Same, on corpus a-prime (corpus a under the exclusion).
row_detect_aprime_at() {
  local name="$1" detect="$2" threads="${3:-}"
  local -a envs=(
    "SCAP_ROOT=$ROOT_A"
    "SCAP_LIST_EXCLUDE=$APRIME_EXCLUDE"
    "SCAP_LIST_DETECT=$detect"
  )
  [[ -n "$threads" ]] && envs+=("SCAP_LIST_THREADS=$threads")
  run_bench "$name" "$ENV_BIN" "${envs[@]}" "$SCAP_BIN" list
}

# --- W3.0b strategy matrix at N* = 4 (the pool's default) ------------------

row_list_aprime_open() { row_detect_aprime_at list_aprime_open open; }
row_list_aprime_stat() { row_detect_aprime_at list_aprime_stat stat; }

row_list_a_open() { row_detect_at list_a_open "$ROOT_A" open; }
row_list_a_stat() { row_detect_at list_a_stat "$ROOT_A" stat; }

row_list_ab_open() { row_detect_at list_ab_open "$ROOT_AB" open; }
row_list_ab_stat() { row_detect_at list_ab_stat "$ROOT_AB" stat; }

# --- Thread sweep on a+b, for the winning strategy -------------------------
#
# Both strategies' rows are defined so the file describes the whole matrix
# regardless of which one W3.0b freezes; SCAP_BENCH_ROWS selects the four
# that are actually run. N = 4 is spelled explicitly here rather than reusing
# the strategy row above, so the sweep's four points are produced by one
# command shape.

row_list_ab_open_t1() { row_detect_at list_ab_open_t1 "$ROOT_AB" open 1; }
row_list_ab_open_t2() { row_detect_at list_ab_open_t2 "$ROOT_AB" open 2; }
row_list_ab_open_t4() { row_detect_at list_ab_open_t4 "$ROOT_AB" open 4; }
row_list_ab_open_t8() { row_detect_at list_ab_open_t8 "$ROOT_AB" open 8; }

row_list_ab_stat_t1() { row_detect_at list_ab_stat_t1 "$ROOT_AB" stat 1; }
row_list_ab_stat_t2() { row_detect_at list_ab_stat_t2 "$ROOT_AB" stat 2; }
row_list_ab_stat_t4() { row_detect_at list_ab_stat_t4 "$ROOT_AB" stat 4; }
row_list_ab_stat_t8() { row_detect_at list_ab_stat_t8 "$ROOT_AB" stat 8; }

# --- Thread sweep on the shipped default -----------------------------------
#
# The sweep above pins a strategy on the command so the two arms of the W3.0b
# comparison are spelled the same way. Once a default is frozen, the sweep
# that re-derives `N*` for the shipped program must instead set *only* the
# thread count and let the default stand, or it measures a configuration no
# user runs.

row_threads_at() {
  local name="$1" rootval="$2" threads="$3"
  run_bench "$name" "$ENV_BIN" "SCAP_ROOT=$rootval" "SCAP_LIST_THREADS=$threads" \
    "$SCAP_BIN" list
}

row_list_ab_t1() { row_threads_at list_ab_t1 "$ROOT_AB" 1; }
row_list_ab_t2() { row_threads_at list_ab_t2 "$ROOT_AB" 2; }
row_list_ab_t4() { row_threads_at list_ab_t4 "$ROOT_AB" 4; }
row_list_ab_t8() { row_threads_at list_ab_t8 "$ROOT_AB" 8; }

# --- AC-6 startup guard ----------------------------------------------------
#
# Same definition as extra-rows-phase-2.sh's, repeated so a Phase-3 group can
# take its AC-6 rows and its list rows from one sourced file.
EMPTY_BIN="${EMPTY_BIN:-$REPO_ROOT/.omc/spikes/w00-empty/target/release/empty}"

row_empty() { run_bench empty "$EMPTY_BIN"; }
