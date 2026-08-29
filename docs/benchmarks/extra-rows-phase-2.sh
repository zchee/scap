#!/usr/bin/env bash
# Extra row_* functions for scripts/bench-quiet.sh, selected via
# SCAP_BENCH_EXTRA + SCAP_BENCH_ROWS. Sourced by the harness, so ENV_BIN,
# SCAP_BIN, PINNED_GITCONFIG and run_bench all come from it; this file is not
# executable on its own.
#
# shellcheck shell=bash

# AC-6's reference: an empty `fn main() {}` built with scap's release profile.
# Same definition (and same default path) as the Phase-0 driver's copy, kept
# here so a committed run is reproducible from committed files alone.
EMPTY_BIN="${EMPTY_BIN:-$REPO_ROOT/.omc/spikes/w00-empty/target/release/empty}"

row_empty() {
  run_bench empty "$EMPTY_BIN"
}

# AC-1's `list` clause: "`scap list` on a GIT_CONFIG_GLOBAL fixture containing
# only `scap.root = a` and `scap.root = b` (no url sections) vs
# `SCAP_ROOT=a:b`: CPU difference <= 1 ms" (plan section 9). The SCAP_ROOT leg
# is the harness's own `list_ab` row; only the pinned leg is new. The fixture
# is the harness's $PINNED_GITCONFIG -- exactly two plain `scap.root` values,
# no url-scoped sections -- read under the same GIT_CONFIG_NOSYSTEM=1 as the
# AC-1 `root_pinned_*` rows, so both AC-1 clauses measure one fixture.
row_list_ab_pinned() {
  run_bench list_ab_pinned "$ENV_BIN" -u SCAP_ROOT \
    "GIT_CONFIG_GLOBAL=$PINNED_GITCONFIG" GIT_CONFIG_NOSYSTEM=1 \
    "$SCAP_BIN" list
}
