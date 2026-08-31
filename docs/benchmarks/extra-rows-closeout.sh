#!/usr/bin/env bash
# Extra row_* functions for scripts/bench-quiet.sh, selected via
# SCAP_BENCH_EXTRA + SCAP_BENCH_ROWS. Sourced by the harness, so ENV_BIN,
# SCAP_BIN, REPO_ROOT, ROOT_A, ROOT_B, ROOT_AB and run_bench all come from it;
# this file is not executable on its own.
#
# These are the Phase-5 closeout rows (plan §6 W5.1, ledger #31). The document
# they feed is a RECORD, not a re-gate: no earlier phase's verdict is
# re-derived here, and nothing in this file is a bound.
#
# Every phase's binary is measured in the SAME WINDOW as HEAD, alternating
# HEAD, reference, HEAD, reference, for the reason the Phase-3 re-gate
# established: this host's wall figures moved ~6 % between 2026-08-28 and
# 2026-08-29 while the same binary's kernel work did not, so a frozen absolute
# and a reading taken today are not the same kind of number. A ratio measured
# inside one window survives that; an absolute quoted across windows does not.
# The alternation also makes drift WITHIN a group visible as a difference
# between the two readings of one binary, instead of being charged to
# whichever binary happened to run second.
#
# The references are the preserved in-place builds under .omc/bench/binaries/,
# each pinned by sha256 in SHA256SUMS there and in the closeout document. They
# are never rebuilt here: a rebuild at a different path yields a different
# crate-metadata hash and therefore a different program, and ledger #22
# recorded that rebuilds on this host do not reproduce byte-for-byte anyway.
#
# shellcheck shell=bash

BIN_DIR="${SCAP_CLOSEOUT_BIN_DIR:-$REPO_ROOT/.omc/bench/binaries}"

# Phase-3 end `0b352d2`, the shipped rustix walker before the index;
# sha256 86dd283519696d44...
REF_P3="${SCAP_CLOSEOUT_REF_P3:-$BIN_DIR/scap-86dd2835}"
# Phase-2b end, the last jwalk walker and AC-3d's reference;
# sha256 0b48c24a65ce86f1...
REF_W2B1="${SCAP_CLOSEOUT_REF_W2B1:-$BIN_DIR/scap-w2b1-inplace}"
# Phase-2 end `47223c2`, in-process config on the old walker;
# sha256 6852d610ba5a29a0...
REF_P2="${SCAP_CLOSEOUT_REF_P2:-$BIN_DIR/scap-phase2-inplace}"
# W1.4 `6ab9038`, the hygiene/dependency-trim tree; sha256 64a67fb54bf4d1f7...
REF_W14="${SCAP_CLOSEOUT_REF_W14:-$BIN_DIR/scap-6ab9038-inplace}"
# The W0.2 `b2-rustix` spike, AC-3c's reference; sha256 73f31c1253fede1e...
SPIKE_BIN="${SCAP_CLOSEOUT_SPIKE:-$REPO_ROOT/.omc/spikes/w02-walker/target/release/spikewalk}"
# The W0.0 empty binary (`fn main`), the process-startup floor.
EMPTY_BIN="${EMPTY_BIN:-$REPO_ROOT/.omc/spikes/w00-empty/target/release/empty}"

# Corpus a′ is corpus a under this exclusion. The harness's inventory section
# reads $APRIME_EXCLUDE, so it is a plain assignment rather than a local.
APRIME_EXCLUDE="${SCAP_BENCH_APRIME_EXCLUDE:-github.com/zchee/claude-code.bak}"

# ---------------------------------------------------------------------------
# Row shapes
# ---------------------------------------------------------------------------
#
# Nothing below sets SCAP_LIST_DETECT, SCAP_LIST_THREADS or SCAP_LIST_CACHE:
# every binary is measured in the configuration it ships, which is the only
# configuration all five of them have in common. Spelling a knob would compare
# a configured HEAD against a reference that cannot read the variable.
#
# Output identity was verified before any timed row and is reported in the
# document: all five scap binaries emit byte-identical listings on a, b and
# a+b (sha256 074bbdd4… / 512c283d… / 3a080dfa…, the same three hashes the
# Phase-3 AC-4 check and the Phase-4b V-6 check recorded). The rows therefore
# compare programs doing the same work, not programs producing different
# answers at different speeds.

row_version_at() {
    local name="$1" bin="$2"
    run_bench "$name" "$bin" --version
}

row_root_env_at() {
    local name="$1" bin="$2"
    run_bench "$name" "$ENV_BIN" "SCAP_ROOT=$ROOT_A" "$bin" root
}

row_list_at_root() {
    local name="$1" bin="$2" root="$3"
    run_bench "$name" "$ENV_BIN" "SCAP_ROOT=$root" "$bin" list
}

row_list_aprime_at() {
    local name="$1" bin="$2"
    run_bench "$name" "$ENV_BIN" "SCAP_ROOT=$ROOT_A" \
        "SCAP_LIST_EXCLUDE=$APRIME_EXCLUDE" "$bin" list
}

# The spike takes its roots positionally and never reads SCAP_ROOT.
# SPIKEWALK_FD_CAP is unset on the command exactly as the W0.2 driver ran it,
# so the row measures the configuration the frozen figures came from.
row_spike_at() {
    local name="$1"
    shift
    run_bench "$name" "$ENV_BIN" -u SPIKEWALK_FD_CAP "$SPIKE_BIN" \
        --variant b2-rustix --threads 4 "$@"
}

# ---------------------------------------------------------------------------
# Group c1-startup — startup and root, five binaries in one window
# ---------------------------------------------------------------------------

row_empty() { run_bench empty "$EMPTY_BIN"; }

row_ver_head_1() { row_version_at ver_head_1 "$SCAP_BIN"; }
row_ver_head_2() { row_version_at ver_head_2 "$SCAP_BIN"; }
row_ver_p3() { row_version_at ver_p3 "$REF_P3"; }
row_ver_w2b1() { row_version_at ver_w2b1 "$REF_W2B1"; }
row_ver_p2() { row_version_at ver_p2 "$REF_P2"; }
row_ver_w14() { row_version_at ver_w14 "$REF_W14"; }

row_rootenv_head_1() { row_root_env_at rootenv_head_1 "$SCAP_BIN"; }
row_rootenv_head_2() { row_root_env_at rootenv_head_2 "$SCAP_BIN"; }
row_rootenv_p3() { row_root_env_at rootenv_p3 "$REF_P3"; }
row_rootenv_w2b1() { row_root_env_at rootenv_w2b1 "$REF_W2B1"; }
row_rootenv_p2() { row_root_env_at rootenv_p2 "$REF_P2"; }
row_rootenv_w14() { row_root_env_at rootenv_w14 "$REF_W14"; }

# ---------------------------------------------------------------------------
# Groups c2-ab / c3-a / c4-b-aprime-depth — `list` on a+b, a and b
# ---------------------------------------------------------------------------
#
# Named so the harness's corpus-inventory matcher fires: it selects corpus a
# on `*list_a*`, b on `*list_b*` and a+b on `*list_ab*`, so what each group
# actually walked is recorded beside its rows rather than assumed.

row_list_ab_head_1() { row_list_at_root list_ab_head_1 "$SCAP_BIN" "$ROOT_AB"; }
row_list_ab_head_2() { row_list_at_root list_ab_head_2 "$SCAP_BIN" "$ROOT_AB"; }
row_list_ab_head_3() { row_list_at_root list_ab_head_3 "$SCAP_BIN" "$ROOT_AB"; }
row_list_ab_p3() { row_list_at_root list_ab_p3 "$REF_P3" "$ROOT_AB"; }
row_list_ab_w2b1() { row_list_at_root list_ab_w2b1 "$REF_W2B1" "$ROOT_AB"; }
row_list_ab_p2() { row_list_at_root list_ab_p2 "$REF_P2" "$ROOT_AB"; }
row_list_ab_w14() { row_list_at_root list_ab_w14 "$REF_W14" "$ROOT_AB"; }
row_list_ab_spike() { row_spike_at list_ab_spike "$ROOT_A" "$ROOT_B"; }

row_list_a_head_1() { row_list_at_root list_a_head_1 "$SCAP_BIN" "$ROOT_A"; }
row_list_a_head_2() { row_list_at_root list_a_head_2 "$SCAP_BIN" "$ROOT_A"; }
row_list_a_head_3() { row_list_at_root list_a_head_3 "$SCAP_BIN" "$ROOT_A"; }
row_list_a_p3() { row_list_at_root list_a_p3 "$REF_P3" "$ROOT_A"; }
row_list_a_w2b1() { row_list_at_root list_a_w2b1 "$REF_W2B1" "$ROOT_A"; }
row_list_a_p2() { row_list_at_root list_a_p2 "$REF_P2" "$ROOT_A"; }
row_list_a_w14() { row_list_at_root list_a_w14 "$REF_W14" "$ROOT_A"; }
row_list_a_spike() { row_spike_at list_a_spike "$ROOT_A"; }

row_list_b_head_1() { row_list_at_root list_b_head_1 "$SCAP_BIN" "$ROOT_B"; }
row_list_b_head_2() { row_list_at_root list_b_head_2 "$SCAP_BIN" "$ROOT_B"; }
row_list_b_head_3() { row_list_at_root list_b_head_3 "$SCAP_BIN" "$ROOT_B"; }
row_list_b_p3() { row_list_at_root list_b_p3 "$REF_P3" "$ROOT_B"; }
row_list_b_w2b1() { row_list_at_root list_b_w2b1 "$REF_W2B1" "$ROOT_B"; }
row_list_b_p2() { row_list_at_root list_b_p2 "$REF_P2" "$ROOT_B"; }
row_list_b_w14() { row_list_at_root list_b_w14 "$REF_W14" "$ROOT_B"; }
row_list_b_spike() { row_spike_at list_b_spike "$ROOT_B"; }

# ---------------------------------------------------------------------------
# `list` on a' (ran inside group c4-b-aprime-depth)
# ---------------------------------------------------------------------------
#
# Only the binaries that implement the exclusion appear here. `SCAP_LIST_EXCLUDE`
# ships in Phase 2b, so the Phase-2 end and W1.4 binaries silently IGNORE it:
# a row named a′ on either of them would walk corpus a in full and publish the
# result under the wrong corpus. Verified rather than assumed — under
# SCAP_LOG=debug the exclusion takes `dirs_read` from 16,933 to 1,198 on HEAD,
# the Phase-3 end and the Phase-2b end binaries, while the Phase-2 end and
# W1.4 binaries emit no walk span at all (they predate it) and their listing is
# unchanged by the variable. Those two corpus-a′ cells are reported as
# unmeasurable, not filled with a corpus-a number.
#
# The listing itself is IDENTICAL on a and a′ (845 repositories either way),
# because the excluded subtree holds no repository; the exclusion's effect is
# 15,735 directory reads, not output. That is why this row is a walk-cost row
# and not an output-parity row.

row_list_aprime_head_1() { row_list_aprime_at list_aprime_head_1 "$SCAP_BIN"; }
row_list_aprime_head_2() { row_list_aprime_at list_aprime_head_2 "$SCAP_BIN"; }
row_list_aprime_head_3() { row_list_aprime_at list_aprime_head_3 "$SCAP_BIN"; }
row_list_aprime_p3() { row_list_aprime_at list_aprime_p3 "$REF_P3"; }
row_list_aprime_w2b1() { row_list_aprime_at list_aprime_w2b1 "$REF_W2B1"; }
row_list_aprime_spike() {
    row_spike_at list_aprime_spike --exclude "$APRIME_EXCLUDE" "$ROOT_A"
}

# ---------------------------------------------------------------------------
# cwd depth (plan §6 W5.1, AC-1's shape; ran inside group c4-b-aprime-depth)
# ---------------------------------------------------------------------------
#
# The harness already ships root_pinned_root_cwd / _outside8_cwd / _inside8_cwd
# on $SCAP_BIN against its own AC-1 fixture; these add the same three shapes on
# the reference binaries, so the cwd-depth row is a per-phase row like the rest
# rather than a HEAD-only reading. The fixture, the pinned GIT_CONFIG_GLOBAL
# and the two 8-deep directories all come from the harness.

row_root_pinned_bin_at() {
    local name="$1" bin="$2" cwd="$3"
    run_bench "$name" "$ENV_BIN" -u SCAP_ROOT -C "$cwd" \
        "GIT_CONFIG_GLOBAL=$PINNED_GITCONFIG" GIT_CONFIG_NOSYSTEM=1 \
        "$bin" root
}

row_depth_root_head() { row_root_pinned_bin_at depth_root_head "$SCAP_BIN" /; }
row_depth_out8_head() { row_root_pinned_bin_at depth_out8_head "$SCAP_BIN" "$CWD_OUTSIDE8"; }
row_depth_in8_head() { row_root_pinned_bin_at depth_in8_head "$SCAP_BIN" "$CWD_INSIDE8"; }
row_depth_root_p3() { row_root_pinned_bin_at depth_root_p3 "$REF_P3" /; }
row_depth_out8_p3() { row_root_pinned_bin_at depth_out8_p3 "$REF_P3" "$CWD_OUTSIDE8"; }
row_depth_in8_p3() { row_root_pinned_bin_at depth_in8_p3 "$REF_P3" "$CWD_INSIDE8"; }
row_depth_root_w14() { row_root_pinned_bin_at depth_root_w14 "$REF_W14" /; }
row_depth_out8_w14() { row_root_pinned_bin_at depth_out8_w14 "$REF_W14" "$CWD_OUTSIDE8"; }
row_depth_in8_w14() { row_root_pinned_bin_at depth_in8_w14 "$REF_W14" "$CWD_INSIDE8"; }

# ---------------------------------------------------------------------------
# Group c8-mimalloc — the mimalloc decision row (plan §10)
# ---------------------------------------------------------------------------
#
# Paired against HEAD in one window. $MIMALLOC_BIN is a scratch build made
# OUTSIDE this worktree so that src/ on main is never touched; when it is
# absent these two rows are simply not selected and the document records the
# decision as not measured, with the reason.

MIMALLOC_BIN="${SCAP_CLOSEOUT_MIMALLOC:-}"

row_mi_head_1() { row_list_at_root mi_head_1 "$SCAP_BIN" "$ROOT_AB"; }
row_mi_head_2() { row_list_at_root mi_head_2 "$SCAP_BIN" "$ROOT_AB"; }
row_mi_mimalloc_1() { row_list_at_root mi_mimalloc_1 "$MIMALLOC_BIN" "$ROOT_AB"; }
row_mi_mimalloc_2() { row_list_at_root mi_mimalloc_2 "$MIMALLOC_BIN" "$ROOT_AB"; }

# ---------------------------------------------------------------------------
# Group c7-loaded — the loaded-machine row (plan §6 W5.1)
# ---------------------------------------------------------------------------
#
# Named to match the Phase-0 baseline's own loaded rows
# (`docs/benchmarks/runs/20260829T132700Z-p0b-loaded`) so the two are directly
# comparable, and kept separate from the quiet rows of the same commands so a
# reader cannot mistake one for the other. This is the ONE row set for which
# SCAP_BENCH_FORCE=1 is legitimate: it exists to measure a deliberately loaded
# host, so gating it on quiet would be gating away the measurement. The load
# is the baseline's own recipe -- eight `yes > /dev/null` spinners -- and the
# harness records the resulting idle, busiest-process and load-average
# figures at both ends without judging them.

row_loaded_root_env() {
    run_bench loaded_root_env "$ENV_BIN" "SCAP_ROOT=$ROOT_A" "$SCAP_BIN" root
}

row_loaded_list_a() {
    run_bench loaded_list_a "$ENV_BIN" "SCAP_ROOT=$ROOT_A" "$SCAP_BIN" list
}

row_loaded_list_ab() {
    run_bench loaded_list_ab "$ENV_BIN" "SCAP_ROOT=$ROOT_AB" "$SCAP_BIN" list
}

# The control arm for the mimalloc row: the SAME source at the SAME path,
# built without the allocator. HEAD's own binary is not the control -- it is
# built at a different path, so its crate-metadata hash differs and comparing
# against it would confound the allocator with the rebuild (ledger #22).
CONTROL_BIN="${SCAP_CLOSEOUT_CONTROL:-}"

row_mi_control_1() { row_list_at_root mi_control_1 "$CONTROL_BIN" "$ROOT_AB"; }
row_mi_control_2() { row_list_at_root mi_control_2 "$CONTROL_BIN" "$ROOT_AB"; }

# ---------------------------------------------------------------------------
# Group c9-cleanflags — the clean-flags row set (deviation D-4)
# ---------------------------------------------------------------------------
#
# `RUSTFLAGS` unset, plain release profile, so the figures a README may quote
# are not native-tuned. Informational, NEVER a gate: no §9 criterion is
# evaluated against them, because every §9 absolute is keyed to the frozen
# regime and a clean-flags reading does not satisfy a criterion it was not
# measured under.
#
# The frozen-regime binary is measured BESIDE the clean one in the same
# window rather than quoted from an earlier group, for the same reason every
# other comparison here is same-window: this host moved ~8 % between
# 2026-08-29 and today, so a cross-window difference would report the machine
# rather than the flags. $FROZEN_BIN is the preserved copy under
# .omc/bench/binaries/, because the in-place rebuild for this row set
# overwrites target/release/scap.
FROZEN_BIN="${SCAP_CLOSEOUT_FROZEN:-$BIN_DIR/scap-92d66004-closeout}"

row_cf_ver_clean() { row_version_at cf_ver_clean "$SCAP_BIN"; }
row_cf_ver_frozen() { row_version_at cf_ver_frozen "$FROZEN_BIN"; }
row_cf_root_clean() { row_root_env_at cf_root_clean "$SCAP_BIN"; }
row_cf_root_frozen() { row_root_env_at cf_root_frozen "$FROZEN_BIN"; }
row_cf_list_ab_clean_1() { row_list_at_root cf_list_ab_clean_1 "$SCAP_BIN" "$ROOT_AB"; }
row_cf_list_ab_frozen_1() { row_list_at_root cf_list_ab_frozen_1 "$FROZEN_BIN" "$ROOT_AB"; }
row_cf_list_ab_clean_2() { row_list_at_root cf_list_ab_clean_2 "$SCAP_BIN" "$ROOT_AB"; }
row_cf_list_ab_frozen_2() { row_list_at_root cf_list_ab_frozen_2 "$FROZEN_BIN" "$ROOT_AB"; }
