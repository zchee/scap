#!/usr/bin/env bash
# Extra row_* functions for scripts/bench-quiet.sh, selected via
# SCAP_BENCH_EXTRA + SCAP_BENCH_ROWS. Sourced by the harness, so ENV_BIN,
# SCAP_BIN, ROOT_A, ROOT_B, ROOT_AB and run_bench all come from it; this file
# is not executable on its own.
#
# These are the Phase-3 re-gate's REFERENCE rows (ledger #21e-ref). The
# ensemble windows established that HEAD reads 137-147 ms on a+b against a
# 140.06 ms bound frozen weeks earlier, and a median inside that spread cannot
# say whether the program got slower or the machine did. These rows answer
# that by measuring older binaries in the SAME window as HEAD: if every
# binary reads slower than its own frozen figure by about the same factor, the
# machine moved, and the ratio between them is the quantity that survives.
#
# Rows alternate HEAD, reference, HEAD, reference so that any drift WITHIN the
# group is visible as a difference between the two readings of the same
# binary, rather than being silently attributed to whichever binary happened
# to run second.
#
# The reference binaries are preserved in-place builds, not rebuilt here: a
# rebuild at a different path yields a different crate-metadata hash and so a
# different program, which is why every one is pinned by sha256 in the
# re-gate document and verified before the group runs. They are read from
# $SCAP_REGATE_BIN_DIR (default /tmp/p3-regate), outside the repository,
# because binaries are not committed.
#
# shellcheck shell=bash

BIN_DIR="${SCAP_REGATE_BIN_DIR:-/tmp/p3-regate}"

# Phase-2b end, the pre-Phase-3 walker (jwalk); sha256 0b48c24a65ce86f1...
# Own-window a+b median 135.358 ms (docs/benchmarks/2026-08-29-phase-2b.md).
REF_W2B1="${SCAP_REGATE_REF_W2B1:-$BIN_DIR/scap-w2b1-inplace}"

# W1.2 `78b3212`, sha256 eb75a35e6c9eebe7..., 2,517,888 bytes -- 32 bytes from
# the Phase-0 baseline binary's 2,517,856 and still on the pre-trim dependency
# set, so it is the closest available stand-in for the Phase-0 baseline whose
# own binary (sha fe0dc41e...) was not preserved. It is NOT pre-Phase-1: the
# plan names 78b3212 as W1.2 itself, and the re-gate document says so rather
# than letting the row read as a baseline measurement.
REF_W12="${SCAP_REGATE_REF_W12:-$BIN_DIR/scap-78b3212}"

# `<bin> list` over a+b with no knobs set, which is what every one of these
# binaries ships as its default. Nothing is spelled with SCAP_LIST_DETECT or
# SCAP_LIST_THREADS here: the older binaries predate both variables, and a row
# that sets them would compare a configured HEAD against an unconfigurable
# reference.
row_ref_list_ab_at() {
    local name="$1" bin="$2"
    run_bench "$name" "$ENV_BIN" "SCAP_ROOT=$ROOT_AB" "$bin" list
}

row_ref_head_1() { row_ref_list_ab_at ref_head_1 "$SCAP_BIN"; }
row_ref_head_2() { row_ref_list_ab_at ref_head_2 "$SCAP_BIN"; }
row_ref_w2b1_1() { row_ref_list_ab_at ref_w2b1_1 "$REF_W2B1"; }
row_ref_w2b1_2() { row_ref_list_ab_at ref_w2b1_2 "$REF_W2B1"; }
row_ref_w12() { row_ref_list_ab_at ref_w12 "$REF_W12"; }

# --- W0.2 spike beside HEAD (ledger #21e-spike) ----------------------------
#
# AC-3c's 140.06 ms bound is 1.15x the W0.2 spike's frozen 121.79 ms
# (`b2-rustix` at N=4 on a+b, docs/benchmarks/2026-08-28-baseline.md). HEAD
# reads 500-532 ms of system time on that corpus today against the spike's
# frozen 430.84, and a frozen figure cannot say whether that gap is the
# program or the machine. These rows read the spike in the SAME window as
# HEAD so the two are comparable.
#
# The spike lives under `.omc/`, which is git-ignored, so its binary is pinned
# by sha256 in the re-gate document rather than committed. `SPIKEWALK_FD_CAP`
# is unset on the command exactly as the W0.2 driver does, so the row measures
# the configuration the frozen figure came from; the spike takes its roots as
# positional arguments and does not read SCAP_ROOT.
#
# This is a DIAGNOSTIC against a frozen Phase-0 figure, not a re-derivation of
# any Phase-0 verdict: one group of 30-run rows cannot restate a matrix of 144.
SPIKE_BIN="${SCAP_REGATE_SPIKE:-$REPO_ROOT/.omc/spikes/w02-walker/target/release/spikewalk}"

row_spike_at() {
    local name="$1"
    run_bench "$name" "$ENV_BIN" -u SPIKEWALK_FD_CAP "$SPIKE_BIN" \
        --variant b2-rustix --threads 4 "$ROOT_A" "$ROOT_B"
}

row_spk_head_1() { row_ref_list_ab_at spk_head_1 "$SCAP_BIN"; }
row_spk_head_2() { row_ref_list_ab_at spk_head_2 "$SCAP_BIN"; }
row_spk_spike_1() { row_spike_at spk_spike_1; }
row_spk_spike_2() { row_spike_at spk_spike_2; }

# The a+b figure the 1.15x bound rests on was measured on 2026-08-28 with the
# ORIGINAL spike build; the binary on disk today is #22's rebuild
# (`73f31c12...`), and #22 recorded that the 2026-08-28 artifacts did not
# reproduce byte-for-byte. So "spike today vs its frozen a+b figure" mixes host
# drift with a possible rebuild difference. This row separates them: #22
# measured THIS binary on corpus a at 117.398 ms, and re-reading the same
# binary on the same corpus isolates how far the host has moved since, with
# the program held constant.
row_spk_spike_a() {
    run_bench spk_spike_a "$ENV_BIN" -u SPIKEWALK_FD_CAP "$SPIKE_BIN" \
        --variant b2-rustix --threads 4 "$ROOT_A"
}

row_spk_head_a() { run_bench spk_head_a "$ENV_BIN" "SCAP_ROOT=$ROOT_A" "$SCAP_BIN" list; }
