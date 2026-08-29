#!/usr/bin/env bash
# Extra row_* functions for scripts/bench-quiet.sh, selected via
# SCAP_BENCH_EXTRA + SCAP_BENCH_ROWS. Sourced by the harness, so ENV_BIN,
# SCAP_BIN, ROOT_A, RUNS, OUT, RAN_ROWS, join_cmd, run_bench and
# assert_no_foreign_hyperfine all come from it; this file is not executable
# on its own.
#
# These are the Phase-4b gate rows (ledger #24b): AC-5's warm-cache `list` on
# corpus a', the same listing built from cold each time, and the same listing
# with the index bypassed.
#
# Corpus a' is corpus a under `SCAP_LIST_EXCLUDE`, spelled exactly as the
# W2b.1 and Phase-3 row files spell it, so the three phases measure the same
# corpus. The harness's inventory section reads $APRIME_EXCLUDE, and its row
# name matching keys on the `list_aprime` prefix, so every row below is named
# accordingly.
#
# shellcheck shell=bash

APRIME_EXCLUDE="${SCAP_BENCH_APRIME_EXCLUDE:-github.com/zchee/claude-code.bak}"

# The index file the a' rows read and write. Corpus a and corpus a' share one
# root path, so they share this file name (it carries FNV-1a of the root); the
# exclusion list recorded inside the file is what tells them apart, and an a
# run and an a' run therefore invalidate each other. Every row here is an a'
# row, so within a group the index stays a' 's.
APRIME_CACHE_DIR="${SCAP_BENCH_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/scap}"

# `run_bench` with a per-run `--prepare` command, for the cold row. Identical
# to the harness's own runner in every other respect -- same `-N`, same run
# count, same export path, same foreign-hyperfine watchdog on both sides --
# so a cold row and a warm row differ only in what happens before each run.
# `--warmup 0` is explicit rather than inherited: a warmup run here would
# leave an index behind that the first timed run would then hit.
run_bench_prepared() {
    local name="$1" prepare="$2"
    shift 2
    local cmd
    cmd="$(join_cmd "$@")"
    echo "==> [$name] (prepare: $prepare) $cmd" >&2
    assert_no_foreign_hyperfine "before row $name"
    "$HYPERFINE_BIN" -N \
        --warmup 0 \
        --prepare "$prepare" \
        --runs "$RUNS" \
        --export-json "$OUT/$name.json" \
        --command-name "$name" \
        "$cmd"
    assert_no_foreign_hyperfine "after row $name"
    RAN_ROWS+=("$name")
}

# AC-5's gated row. The index is enabled through `SCAP_LIST_CACHE=1` rather
# than `--cache` because the environment variable is the shipped opt-in an
# ordinary user sets once; the flag is the per-invocation override. The
# harness runs five warmup invocations before the timed ones, so every timed
# run reads an index a previous run of this same row wrote.
row_list_aprime_cache() {
    run_bench list_aprime_cache "$ENV_BIN" "SCAP_ROOT=$ROOT_A" \
        "SCAP_LIST_EXCLUDE=$APRIME_EXCLUDE" SCAP_LIST_CACHE=1 "$SCAP_BIN" list
}

# The cold half of the same row: the cache directory is removed before every
# timed run, so each one walks the corpus in full and writes the index that
# the next `--prepare` then deletes. This is what a first run after an
# install costs, and it is reported beside the warm figure rather than mixed
# into it -- AC-5 bounds the warm one.
row_list_aprime_cold() {
    run_bench_prepared list_aprime_cold "rm -rf $APRIME_CACHE_DIR" \
        "$ENV_BIN" "SCAP_ROOT=$ROOT_A" \
        "SCAP_LIST_EXCLUDE=$APRIME_EXCLUDE" SCAP_LIST_CACHE=1 "$SCAP_BIN" list
}

# The same listing with the index bypassed end to end -- `--no-cache` neither
# reads nor writes it -- so the group brackets what the index buys on this
# corpus. Not a gated row: AC-5 bounds the cached figure alone.
row_list_aprime_nocache() {
    run_bench list_aprime_nocache "$ENV_BIN" "SCAP_ROOT=$ROOT_A" \
        "SCAP_LIST_EXCLUDE=$APRIME_EXCLUDE" "$SCAP_BIN" list --no-cache
}

# --- W0.5 reference rows (the bound's own denominator) ---------------------
#
# AC-5's bound is not an absolute anyone chose: it is 1.25 x the W0.5 spike's
# `w05-stat-aprime-n4` row, which timed the validation sweep this feature
# implements -- one `fstatat` per known path over the 2,877 a' paths at four
# threads -- and read 4.066 ms wall / 5.259 ms CPU on 2026-08-28. A frozen
# figure cannot say whether a gap opened because the program got slower or
# because the host did, so these rows re-read that same spike binary over that
# same path dump in the SAME window as the scap rows. The quantity that
# survives a host that has moved is the ratio between them, measured together.
#
# The spike lives under `.omc/` and its dump under an untracked run directory,
# so neither is committed; both are pinned by sha256 and line count in the
# Phase-4b document. `spikestat` takes the dump as a positional argument and
# reads no environment.
#
# Rows alternate spike, scap, spike, scap so that drift WITHIN the group shows
# up as a difference between the two readings of the same binary rather than
# being attributed to whichever ran second.
W05_APRIME_DUMP="${SCAP_BENCH_W05_APRIME_DUMP:-$REPO_ROOT/docs/benchmarks/runs/20260828T104448Z-w05/aprime.dump}"
SPIKESTAT_BIN="${SCAP_BENCH_SPIKESTAT:-$REPO_ROOT/.omc/spikes/w02-walker/target/release/spikestat}"

row_w05_stat_at() {
    run_bench "$1" "$SPIKESTAT_BIN" --threads 4 "$W05_APRIME_DUMP"
}

row_w05_stat_1() { row_w05_stat_at w05_stat_1; }
row_w05_stat_2() { row_w05_stat_at w05_stat_2; }

row_list_aprime_cache_at() {
    run_bench "$1" "$ENV_BIN" "SCAP_ROOT=$ROOT_A" \
        "SCAP_LIST_EXCLUDE=$APRIME_EXCLUDE" SCAP_LIST_CACHE=1 "$SCAP_BIN" list
}

row_list_aprime_cache_1() { row_list_aprime_cache_at list_aprime_cache_1; }
row_list_aprime_cache_2() { row_list_aprime_cache_at list_aprime_cache_2; }

# --- Startup floor ---------------------------------------------------------
#
# What any `scap` invocation costs before it has done anything: the W0.0 spike
# is a Rust binary whose `main` returns immediately, so `empty` is process
# creation and dynamic linking alone, and `version` adds clap's setup. The
# warm-cache row pays both on top of the validation sweep the W0.5 reference
# row measures, which is why the two are read in the same window here.
# Same definition as extra-rows-phase-2.sh's and extra-rows-phase-3.sh's.
EMPTY_BIN="${EMPTY_BIN:-$REPO_ROOT/.omc/spikes/w00-empty/target/release/empty}"

row_empty() { run_bench empty "$EMPTY_BIN"; }

# --- Paired benefit-ratio rows (restated AC-5) -----------------------------
#
# The restated criterion is a ratio between two rows taken in the SAME group,
# so neither a slow window nor a fast one can move it: whatever the host is
# doing, it is doing it to both rows. These repeat spellings exist so a group
# can alternate cached, bypassed, cached, bypassed and yield two independent
# ratios, with any drift within the group visible as a difference between the
# two readings of the same command rather than attributed to the pairing.
row_list_aprime_nocache_at() {
    run_bench "$1" "$ENV_BIN" "SCAP_ROOT=$ROOT_A" \
        "SCAP_LIST_EXCLUDE=$APRIME_EXCLUDE" "$SCAP_BIN" list --no-cache
}

row_list_aprime_nocache_1() { row_list_aprime_nocache_at list_aprime_nocache_1; }
row_list_aprime_nocache_2() { row_list_aprime_nocache_at list_aprime_nocache_2; }
