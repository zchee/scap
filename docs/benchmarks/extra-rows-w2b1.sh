#!/usr/bin/env bash
# Extra row_* functions for scripts/bench-quiet.sh, selected via
# SCAP_BENCH_EXTRA + SCAP_BENCH_ROWS. Sourced by the harness, so ENV_BIN,
# SCAP_BIN, ROOT_A and run_bench all come from it; this file is not
# executable on its own.
#
# shellcheck shell=bash

# The exclusion pattern the whole a-prime corpus is defined by: on corpus a,
# 15,735 of 16,933 directory reads (93 %) sit under this one non-repository
# subtree, and it contains no repository at all -- so excluding it changes
# the reads and not a byte of the output (plan section 0, "corpus shape a").
APRIME_EXCLUDE="${SCAP_BENCH_APRIME_EXCLUDE:-github.com/zchee/claude-code.bak}"

# Corpus a-prime: corpus a under `SCAP_LIST_EXCLUDE`. AC-9's wall clause is
# read from this row, and so is the a-prime reference triple that plan
# section 6 (W0.1, "a-prime reference (edit 1)") freezes for AC-3a, AC-3d
# and AC-3': the `5c3531f` binary ignores the variable, so this corpus has
# no measurement before W2b.1 and the W2b.1 binary is its definition.
row_list_aprime() {
  run_bench list_aprime "$ENV_BIN" "SCAP_ROOT=$ROOT_A" \
    "SCAP_LIST_EXCLUDE=$APRIME_EXCLUDE" "$SCAP_BIN" list
}
