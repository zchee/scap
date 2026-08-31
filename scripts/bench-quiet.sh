#!/usr/bin/env bash
# W0.1 quiet-machine benchmark harness (.omc/plans/2026-08-28-theoretical-limit-optimization.md §6).
#
# Runs one hyperfine invocation per row, sequentially, and writes per-row
# JSON plus metadata.json and summary.md under $OUT. Refuses to run on a
# loaded machine unless SCAP_BENCH_FORCE=1 (reserved for the plan's
# explicitly "loaded-machine" row).
#
# "Loaded" is measured as CPU contention rather than as the 1-minute load
# average - see deviation D-1 at check_preconditions below, and the
# "Deviations" section of docs/benchmarks/2026-08-28-baseline.md.
#
# The machine is gated at BOTH ends of a group: check_preconditions before the
# first row, and the closing gate after the last one - see "Closing gate"
# below. Exit codes: 1 a usage or environment error, 3 a gate refusal (the
# group did not measure), 4 a closing-gate discard (the group measured, and
# its rows must not be reported).
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
SCAP_BIN="${SCAP_BIN:-$REPO_ROOT/target/release/scap}"
GHQ_BIN="${GHQ_BIN:-$(command -v ghq || true)}"
HYPERFINE_BIN="${HYPERFINE_BIN:-$(command -v hyperfine || true)}"
ENV_BIN="${ENV_BIN:-$(command -v env || true)}"
JQ_BIN="${JQ_BIN:-$(command -v jq || true)}"
GIT_BIN="${GIT_BIN:-$(command -v git || true)}"

WARMUP=5
RUNS="${SCAP_BENCH_RUNS:-30}"
FORCE="${SCAP_BENCH_FORCE:-0}"
OUT="${OUT:-$REPO_ROOT/docs/benchmarks/runs/$(date -u +%Y%m%dT%H%M%SZ)}"

ROOT_A="${SCAP_BENCH_ROOT_A:-/Users/zchee/src}"
ROOT_B="${SCAP_BENCH_ROOT_B:-/Users/zchee/go/src}"
ROOT_AB="$ROOT_A:$ROOT_B"

for bin_name in SCAP_BIN GHQ_BIN HYPERFINE_BIN ENV_BIN JQ_BIN GIT_BIN; do
  if [[ -z "${!bin_name}" ]]; then
    echo "bench-quiet.sh: required tool for \$$bin_name not found on PATH" >&2
    exit 1
  fi
done
if [[ ! -x "$SCAP_BIN" ]]; then
  echo "bench-quiet.sh: SCAP_BIN=$SCAP_BIN is not an executable file (build with: cargo build --release)" >&2
  exit 1
fi
# ---------------------------------------------------------------------------
# Build-flag regime (plan §6a deviation D-4)
# ---------------------------------------------------------------------------
#
# Every Phase-0 binary was built under a global RUSTFLAGS exported by the
# maintainer's login shell, so every frozen §9 absolute is valid only against
# binaries built with that exact string. The string below is the one the
# benchmark panes export -- verified 2026-08-29: reproduces the Phase-0 `scap`
# binary sha256 fe0dc41e49abad1b5592aed6c56537583b8289cdc83660a2bc94d871bb91ca51
# exactly (2,517,856 B, __TEXT 1,884,160 B). Plan §6a deviation D-4 recorded a
# different string ("-C target-cpu=native ... -C panic=abort ..."); that was the
# lead session's own bash value, a transcription error, corrected in the plan
# alongside this change. A run whose RUSTFLAGS differs is measuring a different
# program and is refused unless the caller says otherwise.
FROZEN_RUSTFLAGS='-C target-cpu=apple-m3 -C target-feature=+neon -C opt-level=3 -C codegen-units=1 -C force-frame-pointers=on -C embed-bitcode=yes -Z dylib-lto -Z mir-opt-level=4 -Z inline-mir=yes -C llvm-args=-unroll-threshold=500 -C llvm-args=-enable-dfa-jump-thread -C link-arg=-Wl,-dead_strip'
CURRENT_RUSTFLAGS="${RUSTFLAGS:-}"
ALLOW_FLAGS="${SCAP_BENCH_ALLOW_FLAGS:-0}"

if [[ "$CURRENT_RUSTFLAGS" != "$FROZEN_RUSTFLAGS" ]]; then
  if [[ "$ALLOW_FLAGS" != "1" ]]; then
    {
      echo "bench-quiet.sh: RUSTFLAGS does not match the regime frozen in plan §6a deviation D-4."
      echo "  frozen:  $FROZEN_RUSTFLAGS"
      echo "  current: ${CURRENT_RUSTFLAGS:-<unset>}"
      echo "The §9 absolutes are only valid against binaries built with the frozen string, so a"
      echo "run under different flags does not satisfy the criterion it claims to."
      echo "Set SCAP_BENCH_ALLOW_FLAGS=1 to record the run anyway (metadata.json marks it)."
    } >&2
    exit 1
  fi
  echo "bench-quiet.sh: RUSTFLAGS differs from the D-4 regime; recording anyway (SCAP_BENCH_ALLOW_FLAGS=1)." >&2
fi

if ! "$ENV_BIN" --version 2>/dev/null | grep -q 'GNU coreutils'; then
  echo "bench-quiet.sh: \$ENV_BIN ($ENV_BIN) must be GNU env (needs -C/-u); found: $("$ENV_BIN" --version 2>&1 | head -n1)" >&2
  exit 1
fi

mkdir -p "$OUT"
OUT_REAL="$(cd "$OUT" && pwd -P)"

# Fingerprint of the binary these rows measure (deviation D-4 item 2), so a
# later reader can tell whether two runs measured the same program.
BIN_SIZE_BYTES="$(wc -c < "$SCAP_BIN" | tr -d ' ')"
BIN_SHA256="$(shasum -a 256 "$SCAP_BIN" | awk '{print $1}')"

# ---------------------------------------------------------------------------
# Foreign-hyperfine refusal (ledger #21b / #22 NOTES)
# ---------------------------------------------------------------------------
#
# Two lanes measuring this host at once invalidates both, and the 2026-08-29
# window proved the gate above cannot catch it: the CPU-idle clause admitted a
# group while another lane's hyperfine was running, because one benchmark
# process does not move a 16-core machine's idle figure far enough to fail an
# 85 % floor. The rows were only identified as contaminated afterwards.
#
# The rule is ownership, not activity: this script's own hyperfine always
# writes its `--export-json` under $OUT, so ANY hyperfine whose export path
# lies outside this run's $OUT belongs to somebody else -- including a lane
# exporting elsewhere under this same repository, which is exactly the case
# the earlier `$REPO/docs/benchmarks/runs/` substring test could not separate.
# A hyperfine with no `--export-json` at all cannot prove it is ours and counts
# as foreign too.
#
# File mtimes are deliberately NOT used as an activity signal: restoring git
# history rewrote older run directories onto disk mid-window once already, so
# an mtime says nothing about who is measuring now.
#
# `pgrep -x` matches the executable name. `pgrep -f hyperfine` is wrong here:
# it also matches this script's own shell whenever the word appears anywhere
# on its command line, so the gate would flag itself.
FOREIGN_SCANS=0

# Prints one `pid<TAB>command` line per foreign hyperfine; empty when clean.
# Counting the scan is the caller's job: this runs inside a command
# substitution, so an increment here would be lost with the subshell.
foreign_hyperfine_list() {
  local pid args path path_dir path_real
  while read -r pid; do
    [[ -z "$pid" ]] && continue
    args="$(ps -o args= -p "$pid" 2>/dev/null || true)"
    [[ -z "$args" ]] && continue
    path="$(printf '%s\n' "$args" | awk '
      { for (i = 1; i <= NF; i++) {
          if ($i == "--export-json" && i < NF) { print $(i + 1); exit }
          if ($i ~ /^--export-json=/) { sub(/^--export-json=/, "", $i); print $i; exit }
        } }')"
    if [[ -n "$path" ]]; then
      path_dir="$(dirname "$path")"
      if [[ -d "$path_dir" ]]; then
        path_real="$(cd "$path_dir" && pwd -P)/$(basename "$path")"
      else
        # An export directory that does not exist cannot be under our $OUT,
        # which this script created before the first scan.
        path_real="$path"
      fi
      [[ "$path_real" == "$OUT_REAL"/* ]] && continue
    fi
    printf '%s\t%s\n' "$pid" "$args"
  done < <(pgrep -x hyperfine || true)
}

# Refuses the run when another lane is measuring. Not bypassed by
# SCAP_BENCH_FORCE: forcing declares "I loaded this machine deliberately",
# which is a statement about load, not a licence to measure on top of somebody
# else's benchmark.
assert_no_foreign_hyperfine() {
  local where="$1" found
  found="$(foreign_hyperfine_list)"
  FOREIGN_SCANS=$(( FOREIGN_SCANS + 1 ))
  [[ -z "$found" ]] && return 0
  {
    echo "bench-quiet.sh: refusing to run beside a foreign hyperfine ($where):"
    printf '  - %s\n' "$found"
    echo "A hyperfine whose --export-json is outside this run's OUT ($OUT_REAL)"
    echo "belongs to another lane; both measurements would be contended."
    echo "Discard this run directory and re-take the group once the host is free."
  } >&2
  # Label the directory rather than deleting it: a partial run dir that says
  # why it was abandoned is evidence, and a reader who finds one later must
  # not mistake it for a completed group.
  {
    printf 'Aborted %s: foreign hyperfine detected (%s).\n' \
      "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$where"
    printf '%s\n' "$found"
    printf 'These rows are contaminated and must not be reported.\n'
  } >> "$OUT/CONTAMINATED"
  exit 3
}

# ---------------------------------------------------------------------------
# Quiet-machine preconditions
# ---------------------------------------------------------------------------

xprotectd_cpu_pct() {
  local pids pid cpu total found
  total="0.0"
  found=0
  pids="$(pgrep -x xprotectd || true)"
  for pid in $pids; do
    cpu="$(ps -o %cpu= -p "$pid" 2>/dev/null | tr -d ' ')"
    [[ -z "$cpu" ]] && continue
    total="$(awk -v a="$total" -v b="$cpu" 'BEGIN { printf "%.2f", a + b }')"
    found=1
  done
  if [[ "$found" -eq 0 ]]; then
    printf '0.0'
  else
    printf '%s' "$total"
  fi
}

# One `top` sample yields both the CPU idle percentage and the busiest
# process's *instantaneous* CPU, in SAMPLE_IDLE and SAMPLE_TOP.
#
# `-l 2` is required: the first sample reports usage since boot, only the
# second is an interval measurement, so everything after the last "%CPU"
# header is the live table. `ps -o %cpu` is deliberately not used - it reports
# an average over each process's whole lifetime rather than current activity,
# which reads a long-idle browser as busy and a just-started build as idle.
# `top` and `ps` are excluded from the table so the probe cannot flag itself.
# Processes excluded from the busiest-process scan (deviation D-1a).
# `top` and `ps` are the probe's own commands and must never flag
# themselves. `kernel_task` and `WindowServer` are unavoidable macOS
# infrastructure on any machine with a display: on an idle host they sit
# at 19-23 % and 14-22 % respectively, so leaving them in would make the
# clause fire on the machine's own floor. Neither competes for the
# benchmark's cores the way this clause exists to catch, and both are
# already counted in the idle figure, so excluding them hides no
# contention. What remains is the real target: a user process pegging a
# core while the idle figure stays high.
TOP_PROC_EXCLUDE="${SCAP_BENCH_TOP_PROC_EXCLUDE:-top ps kernel_task WindowServer}"

sample_cpu() {
  local out table
  out="$(top -l 2 -n 8 -o cpu -stats cpu,command 2>/dev/null)"
  SAMPLE_IDLE="$(printf '%s\n' "$out" | grep '^CPU usage' | tail -1 \
    | sed -n 's/.*, *\([0-9.]*\)% idle.*/\1/p')"
  table="$(printf '%s\n' "$out" \
    | awk '/^%CPU/ { buf = ""; next } { buf = buf $0 "\n" } END { printf "%s", buf }')"
  SAMPLE_TOP="$(printf '%s\n' "$table" \
    | awk -v excl="$TOP_PROC_EXCLUDE" '
        BEGIN { n = split(excl, a, " "); for (i = 1; i <= n; i++) skip[a[i]] = 1 }
        NF >= 2 && !($2 in skip) { print $1 }' \
    | sort -g | tail -1)"
  SAMPLE_IDLE="${SAMPLE_IDLE:-0}"
  SAMPLE_TOP="${SAMPLE_TOP:-0}"
}

# Median of three samples taken 5 s apart, so one transient spike neither
# passes nor fails the gate on its own.
sample_cpu_median3() {
  local i idles=() tops=()
  for i in 1 2 3; do
    sample_cpu
    idles+=("$SAMPLE_IDLE")
    tops+=("$SAMPLE_TOP")
    (( i < 3 )) && sleep 5
  done
  SAMPLE_IDLE="$(printf '%s\n' "${idles[@]}" | sort -g | sed -n '2p')"
  SAMPLE_TOP="$(printf '%s\n' "${tops[@]}" | sort -g | sed -n '2p')"
}

# --- Deviation D-1 (approved 2026-08-28) -----------------------------------
# The plan says "1-minute load average < 2.0". The intent behind it is "no CPU
# contention", and on this Darwin host the two are decoupled: measured over 40
# minutes, the machine sat at 89.85 % idle with four runnable threads while the
# 1-minute load average read 5.06, and the load average did not follow Chrome's
# CPU falling from 55 % to 9 %. Gating on the load average would therefore
# refuse every run on an idle machine. The gate below measures contention
# directly instead, and is strictly stronger than the original on the condition
# §0 actually describes (load 18.9, xprotectd scanning, two concurrent
# hyperfine sessions would fail the idle test outright). The load average is
# still sampled and recorded per run, so any row can be audited against it.
IDLE_MIN="${SCAP_BENCH_IDLE_MIN:-85.0}"
TOP_PROC_MAX="${SCAP_BENCH_TOP_PROC_MAX:-15.0}"

LOADAVG_START="$(sysctl -n vm.loadavg)"
XPROTECTD_CPU_START="$(xprotectd_cpu_pct)"
IDLE_START=""
TOP_PROC_START=""

check_preconditions() {
  local failures=()

  # Before the 15 s of CPU sampling, so a contended host is refused
  # immediately rather than after the gate has paid for its samples.
  assert_no_foreign_hyperfine "start-of-run gate"

  sample_cpu_median3
  IDLE_START="$SAMPLE_IDLE"
  TOP_PROC_START="$SAMPLE_TOP"

  if ! awk -v v="$IDLE_START" 'BEGIN { exit !(v >= '"$IDLE_MIN"') }'; then
    failures+=("CPU idle ${IDLE_START}% < ${IDLE_MIN}% (median of 3 samples, top -l 2)")
  fi

  if ! awk -v v="$TOP_PROC_START" 'BEGIN { exit !(v <= '"$TOP_PROC_MAX"') }'; then
    failures+=("busiest process at ${TOP_PROC_START}% CPU > ${TOP_PROC_MAX}% (median of 3 samples, top -o cpu)")
  fi

  if ! awk -v v="$XPROTECTD_CPU_START" 'BEGIN { exit !(v < 5.0) }'; then
    failures+=("xprotectd CPU ${XPROTECTD_CPU_START}% >= 5% (ps -o %cpu= -p \$(pgrep -x xprotectd))")
  fi

  if pgrep -x cargo >/dev/null 2>&1; then
    failures+=("a cargo process is running (pgrep -x cargo)")
  fi
  if pgrep -x rustc >/dev/null 2>&1; then
    failures+=("a rustc process is running (pgrep -x rustc)")
  fi

  if (( ${#failures[@]} > 0 )); then
    echo "bench-quiet.sh: refusing to run on a non-quiet machine:" >&2
    local f
    for f in "${failures[@]}"; do
      echo "  - $f" >&2
    done
    echo "1-minute load average (recorded, not gated - deviation D-1): ${LOADAVG_START}" >&2
    echo "Set SCAP_BENCH_FORCE=1 to override (only for the plan's explicitly \"loaded-machine\" row)." >&2
    exit 3
  fi
}

if [[ "$FORCE" != "1" ]]; then
  check_preconditions
else
  # Forced (loaded-machine rows): record the same numbers without gating. The
  # foreign-hyperfine clause still applies -- see assert_no_foreign_hyperfine.
  assert_no_foreign_hyperfine "start-of-run gate (forced)"
  sample_cpu
  IDLE_START="$SAMPLE_IDLE"
  TOP_PROC_START="$SAMPLE_TOP"
fi

# ---------------------------------------------------------------------------
# Fixtures (built once, under a temp dir, regardless of which rows run)
# ---------------------------------------------------------------------------

FIXTURE_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/scap-bench-fixtures.XXXXXX")"
cleanup() {
  rm -rf "$FIXTURE_ROOT"
}
trap cleanup EXIT

# AC-1 pinned fixture: GIT_CONFIG_GLOBAL + GIT_CONFIG_NOSYSTEM=1, no SCAP_ROOT.
PINNED_GITCONFIG="$FIXTURE_ROOT/pinned-gitconfig"
cat > "$PINNED_GITCONFIG" <<'EOF'
[scap]
	root = /Users/zchee/src
	root = /Users/zchee/go/src
EOF

# An 8-deep cwd outside any git repository.
CWD_OUTSIDE8="$FIXTURE_ROOT/outside/l1/l2/l3/l4/l5/l6/l7/l8"
mkdir -p "$CWD_OUTSIDE8"

# An 8-deep cwd inside a git repo with extensions.worktreeConfig=true and a
# config.worktree file present.
INSIDE_REPO="$FIXTURE_ROOT/inside-repo"
mkdir -p "$INSIDE_REPO"
"$GIT_BIN" -C "$INSIDE_REPO" init -q
"$GIT_BIN" -C "$INSIDE_REPO" config extensions.worktreeConfig true
: > "$INSIDE_REPO/.git/config.worktree"
CWD_INSIDE8="$INSIDE_REPO/d1/d2/d3/d4/d5/d6/d7/d8"
mkdir -p "$CWD_INSIDE8"

# ---------------------------------------------------------------------------
# hyperfine runner (one benchmark per invocation, sequential)
# ---------------------------------------------------------------------------

RAN_ROWS=()

sq() {
  printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

join_cmd() {
  local out="" tok
  for tok in "$@"; do
    out+="${out:+ }$(sq "$tok")"
  done
  printf '%s' "$out"
}

run_bench() {
  local name="$1"
  shift
  local cmd
  cmd="$(join_cmd "$@")"
  echo "==> [$name] $cmd" >&2
  # Watchdog on both sides of the row: before, so a lane that started while
  # the previous row ran cannot contend with this one; after, so a lane that
  # started DURING this row marks it contaminated instead of letting a
  # contended measurement reach summary.md.
  assert_no_foreign_hyperfine "before row $name"
  "$HYPERFINE_BIN" -N \
    --warmup "$WARMUP" \
    --runs "$RUNS" \
    --export-json "$OUT/$name.json" \
    --command-name "$name" \
    "$cmd"
  assert_no_foreign_hyperfine "after row $name"
  RAN_ROWS+=("$name")
}

# ---------------------------------------------------------------------------
# Rows
# ---------------------------------------------------------------------------

row_version() {
  run_bench version "$SCAP_BIN" --version
}

row_root_env() {
  run_bench root_env "$ENV_BIN" "SCAP_ROOT=$ROOT_A" "$SCAP_BIN" root
}

# Shared helper: `scap root`, no SCAP_ROOT, AC-1 pinned fixture, given cwd.
row_root_pinned_at() {
  local name="$1" cwd="$2"
  run_bench "$name" "$ENV_BIN" -u SCAP_ROOT -C "$cwd" \
    "GIT_CONFIG_GLOBAL=$PINNED_GITCONFIG" GIT_CONFIG_NOSYSTEM=1 \
    "$SCAP_BIN" root
}

row_root_pinned_root_cwd() { row_root_pinned_at root_pinned_root_cwd /; }
row_root_pinned_outside8_cwd() { row_root_pinned_at root_pinned_outside8_cwd "$CWD_OUTSIDE8"; }
row_root_pinned_inside8_cwd() { row_root_pinned_at root_pinned_inside8_cwd "$CWD_INSIDE8"; }

# Shared helper for `<bin> list [flags] <root...>` rows.
row_list_at() {
  local name="$1" bin="$2" rootvar="$3" rootval="$4"
  shift 4
  run_bench "$name" "$ENV_BIN" "${rootvar}=${rootval}" "$bin" list "$@"
}

row_list_a() { row_list_at list_a "$SCAP_BIN" SCAP_ROOT "$ROOT_A"; }
row_list_a_p() { row_list_at list_a_p "$SCAP_BIN" SCAP_ROOT "$ROOT_A" -p; }
row_list_a_unique() { row_list_at list_a_unique "$SCAP_BIN" SCAP_ROOT "$ROOT_A" --unique; }
row_list_a_query() { row_list_at list_a_query "$SCAP_BIN" SCAP_ROOT "$ROOT_A" zchee; }

row_list_b() { row_list_at list_b "$SCAP_BIN" SCAP_ROOT "$ROOT_B"; }
row_list_b_p() { row_list_at list_b_p "$SCAP_BIN" SCAP_ROOT "$ROOT_B" -p; }
row_list_b_unique() { row_list_at list_b_unique "$SCAP_BIN" SCAP_ROOT "$ROOT_B" --unique; }
row_list_b_query() { row_list_at list_b_query "$SCAP_BIN" SCAP_ROOT "$ROOT_B" zchee; }

row_list_ab() { row_list_at list_ab "$SCAP_BIN" SCAP_ROOT "$ROOT_AB"; }
row_list_ab_p() { row_list_at list_ab_p "$SCAP_BIN" SCAP_ROOT "$ROOT_AB" -p; }
row_list_ab_unique() { row_list_at list_ab_unique "$SCAP_BIN" SCAP_ROOT "$ROOT_AB" --unique; }
row_list_ab_query() { row_list_at list_ab_query "$SCAP_BIN" SCAP_ROOT "$ROOT_AB" zchee; }

row_ghq_list_a() { row_list_at ghq_list_a "$GHQ_BIN" GHQ_ROOT "$ROOT_A"; }
row_ghq_list_a_p() { row_list_at ghq_list_a_p "$GHQ_BIN" GHQ_ROOT "$ROOT_A" -p; }
row_ghq_list_a_unique() { row_list_at ghq_list_a_unique "$GHQ_BIN" GHQ_ROOT "$ROOT_A" --unique; }
row_ghq_list_a_query() { row_list_at ghq_list_a_query "$GHQ_BIN" GHQ_ROOT "$ROOT_A" zchee; }

row_ghq_list_b() { row_list_at ghq_list_b "$GHQ_BIN" GHQ_ROOT "$ROOT_B"; }
row_ghq_list_b_p() { row_list_at ghq_list_b_p "$GHQ_BIN" GHQ_ROOT "$ROOT_B" -p; }
row_ghq_list_b_unique() { row_list_at ghq_list_b_unique "$GHQ_BIN" GHQ_ROOT "$ROOT_B" --unique; }
row_ghq_list_b_query() { row_list_at ghq_list_b_query "$GHQ_BIN" GHQ_ROOT "$ROOT_B" zchee; }

row_ghq_list_ab() { row_list_at ghq_list_ab "$GHQ_BIN" GHQ_ROOT "$ROOT_AB"; }
row_ghq_list_ab_p() { row_list_at ghq_list_ab_p "$GHQ_BIN" GHQ_ROOT "$ROOT_AB" -p; }
row_ghq_list_ab_unique() { row_list_at ghq_list_ab_unique "$GHQ_BIN" GHQ_ROOT "$ROOT_AB" --unique; }
row_ghq_list_ab_query() { row_list_at ghq_list_ab_query "$GHQ_BIN" GHQ_ROOT "$ROOT_AB" zchee; }

row_ghq_root() {
  run_bench ghq_root "$GHQ_BIN" root
}

DEFAULT_ROWS=(
  version root_env
  root_pinned_root_cwd root_pinned_outside8_cwd root_pinned_inside8_cwd
  list_a list_a_p list_a_unique list_a_query
  list_b list_b_p list_b_unique list_b_query
  list_ab list_ab_p list_ab_unique list_ab_query
  ghq_list_a ghq_list_a_p ghq_list_a_unique ghq_list_a_query
  ghq_list_b ghq_list_b_p ghq_list_b_unique ghq_list_b_query
  ghq_list_ab ghq_list_ab_p ghq_list_ab_unique ghq_list_ab_query
  ghq_root
)

# SCAP_BENCH_EXTRA sources a file that may define additional row_<name>
# functions (spike binaries); select them explicitly via SCAP_BENCH_ROWS.
if [[ -n "${SCAP_BENCH_EXTRA:-}" ]]; then
  # shellcheck source=/dev/null
  source "$SCAP_BENCH_EXTRA"
fi

if [[ -n "${SCAP_BENCH_ROWS:-}" ]]; then
  IFS=',' read -r -a SELECTED_ROWS <<< "$SCAP_BENCH_ROWS"
else
  SELECTED_ROWS=("${DEFAULT_ROWS[@]}")
fi

# ---------------------------------------------------------------------------
# Corpus inventory (recorded, never gated)
# ---------------------------------------------------------------------------
#
# The corpora are live directories on the maintainer's machine, not fixtures.
# Between the 2026-08-28 freeze and W2b.1 corpus a gained four repositories
# and two owner directories, so a run that does not say what it walked cannot
# honestly be compared with one taken a week earlier. Every row set that
# touches a corpus now records what was actually there, next to the frozen
# reference, in metadata.json.
#
# One `scap list` per corpus, before the timed rows, so the inventory never
# runs between two hyperfine invocations.
#
# `dirs_read` is NOT comparable across walkers and no drift should be read
# from it: the W0.2 spike finds `.git` by reading a repository's directory,
# while the jwalk walker finds it with a `stat` and never opens it, so the
# spike counts one extra read per repository it opens (840 of its 841 on
# corpus a -- the exception is a symlinked repository it emits without
# descending, ADR-9 rule (iii)). `repos` is directly comparable and is the
# drift signal; `repos_drift` reports it.
INVENTORY="${SCAP_BENCH_INVENTORY:-auto}"
INVENTORY_JSON='[]'

# One `corpus|root|exclude|frozen_dirs_read|frozen_repos` line per corpus the
# selected rows walk. The frozen figures are the W0.2 spike counters frozen
# in the task ledger (#2) on 2026-08-28.
inventory_specs() {
  local want_a=0 want_b=0 want_aprime=0 row
  for row in "${SELECTED_ROWS[@]}"; do
    case "$row" in
      list_aprime*) want_aprime=1 ;;
      *list_ab*) want_a=1; want_b=1 ;;
      *list_a*) want_a=1 ;;
      *list_b*) want_b=1 ;;
    esac
  done
  (( want_a )) && printf 'a|%s||17771|841\n' "$ROOT_A"
  (( want_aprime )) && printf "a'|%s|%s|2036|841\n" "$ROOT_A" "${APRIME_EXCLUDE:-}"
  (( want_b )) && printf 'b|%s||3589|981\n' "$ROOT_B"
  return 0
}

# `scap list` on one corpus, reporting `repos` from stdout and the walk
# counters from the `SCAP_LOG=debug` span close line. A binary older than
# W2b.1 emits no `scap::walk::root` span, so both counters record as null
# rather than failing the run.
inventory_entry() {
  local name="$1" root="$2" exclude="$3" fz_dirs="$4" fz_repos="$5"
  local stdout_file stderr_file repos dirs excluded
  stdout_file="$(mktemp "${TMPDIR:-/tmp}/scap-inv.XXXXXX")"
  stderr_file="$(mktemp "${TMPDIR:-/tmp}/scap-inv.XXXXXX")"
  if [[ -n "$exclude" ]]; then
    SCAP_LOG=debug SCAP_ROOT="$root" SCAP_LIST_EXCLUDE="$exclude" \
      "$SCAP_BIN" list >"$stdout_file" 2>"$stderr_file" || true
  else
    SCAP_LOG=debug SCAP_ROOT="$root" \
      "$SCAP_BIN" list >"$stdout_file" 2>"$stderr_file" || true
  fi
  repos="$(wc -l < "$stdout_file" | tr -d ' ')"
  # Quit at the first match inside sed rather than piping to `head -1`:
  # under `pipefail` the SIGPIPE that `head` sends once it has its line
  # makes the whole pipeline report failure.
  dirs="$(sed -n '/dirs_read=/{s/.*dirs_read=\([0-9][0-9]*\).*/\1/p;q;}' "$stderr_file")"
  excluded="$(sed -n '/excluded=/{s/.*excluded=\([0-9][0-9]*\).*/\1/p;q;}' "$stderr_file")"
  rm -f "$stdout_file" "$stderr_file"

  # shellcheck disable=SC2016 # single-quoted jq program; $vars are jq bindings
  "$JQ_BIN" -n \
    --arg corpus "$name" \
    --arg root "$root" \
    --arg exclude "$exclude" \
    --arg repos "$repos" \
    --arg dirs "${dirs:-}" \
    --arg excluded "${excluded:-}" \
    --arg fz_dirs "$fz_dirs" \
    --arg fz_repos "$fz_repos" \
    '{
      corpus: $corpus,
      root: $root,
      exclude: (if $exclude == "" then null else $exclude end),
      measured: {
        repos: ($repos | tonumber),
        dirs_read: (if $dirs == "" then null else ($dirs | tonumber) end),
        excluded: (if $excluded == "" then null else ($excluded | tonumber) end)
      },
      frozen_2026_08_28: {
        dirs_read: ($fz_dirs | tonumber),
        repos: ($fz_repos | tonumber),
        source: "W0.2 spike counters (ledger #2)"
      },
      repos_drift: (($repos | tonumber) - ($fz_repos | tonumber)),
      note: "dirs_read is not comparable with the frozen figure across walkers; repos_drift is the drift signal"
    }'
}

collect_inventory() {
  local specs entries=() name root exclude fz_dirs fz_repos
  specs="$(inventory_specs)"
  [[ -z "$specs" ]] && return 0
  while IFS='|' read -r name root exclude fz_dirs fz_repos; do
    [[ -z "$name" ]] && continue
    echo "==> [inventory] corpus $name ($root)" >&2
    entries+=("$(inventory_entry "$name" "$root" "$exclude" "$fz_dirs" "$fz_repos")")
  done <<< "$specs"
  (( ${#entries[@]} == 0 )) && return 0
  INVENTORY_JSON="$(printf '%s\n' "${entries[@]}" | "$JQ_BIN" -s '.')"
}

case "$INVENTORY" in
  0) ;;
  1 | auto) collect_inventory ;;
  *)
    echo "bench-quiet.sh: SCAP_BENCH_INVENTORY must be 0, 1 or auto (got '$INVENTORY')" >&2
    exit 1
    ;;
esac

for row in "${SELECTED_ROWS[@]}"; do
  fn="row_$row"
  if ! declare -F "$fn" >/dev/null; then
    echo "bench-quiet.sh: unknown row '$row' (no function $fn)" >&2
    exit 1
  fi
  "$fn"
done

# ---------------------------------------------------------------------------
# Closing gate (plan §6 W5.1 item (b); ledger #21e HANDOFF)
# ---------------------------------------------------------------------------
#
# The start gate alone admits a group that BEGINS quiet and ENDS contended,
# whose later rows were therefore measured against a competitor. Until now the
# harness only RECORDED the closing sample and the Phase-3 re-gate lane
# enforced it in an operator wrapper (.omc/bench/p3-regate/group.sh), which is
# what discarded 11 of that lane's groups; the Phase-4b gate discarded two
# more. A rule that decides which rows may be published belongs in the harness
# rather than in each lane's private wrapper, so the check runs here.
#
# Same two clauses, same constants and the same median of three samples as the
# start gate: a close held to a weaker bar than the start would admit exactly
# the drift it exists to catch. xprotectd, cargo and rustc are start-only
# clauses and stay that way - the closing rule is the one the re-gate and
# Phase-4b lanes actually applied (ledger #21e HANDOFF names cpu_idle_pct.end
# and top_proc_cpu_pct.end), and widening it here would silently reinterpret
# the discards already on the record.
#
# There is deliberately no --no-closing-gate: a lane able to switch the check
# off after seeing its numbers is choosing its own windows again.
# SCAP_BENCH_FORCE=1 is not that switch - it declares the machine
# deliberately loaded, so the close is recorded and NOT gated, exactly as the
# start is. Under FORCE the closing sample stays a single `top` reading, which
# is what the forced start takes.
#
# A failing group is DISCARDED, never deleted: metadata.json (with
# closing_gate.passed = false and the failing clauses), summary.md and every
# row JSON are written first, then a DISCARDED marker, then exit 4. The
# numbers survive so the discard can be disclosed in the record instead of
# reading as a window quietly dropped.

LOADAVG_END="$(sysctl -n vm.loadavg)"
XPROTECTD_CPU_END="$(xprotectd_cpu_pct)"

CLOSING_FAILURES=()
if [[ "$FORCE" != "1" ]]; then
  CLOSING_CHECKED=1
  sample_cpu_median3
else
  CLOSING_CHECKED=0
  sample_cpu
fi
IDLE_END="$SAMPLE_IDLE"
TOP_PROC_END="$SAMPLE_TOP"

if (( CLOSING_CHECKED )); then
  if ! awk -v v="$IDLE_END" 'BEGIN { exit !(v >= '"$IDLE_MIN"') }'; then
    CLOSING_FAILURES+=("CPU idle ${IDLE_END}% < ${IDLE_MIN}% (median of 3 samples, top -l 2)")
  fi
  if ! awk -v v="$TOP_PROC_END" 'BEGIN { exit !(v <= '"$TOP_PROC_MAX"') }'; then
    CLOSING_FAILURES+=("busiest process at ${TOP_PROC_END}% CPU > ${TOP_PROC_MAX}% (median of 3 samples, top -o cpu)")
  fi
fi

if (( ${#CLOSING_FAILURES[@]} > 0 )); then
  CLOSING_FAILURES_JSON="$(printf '%s\n' "${CLOSING_FAILURES[@]}" \
    | "$JQ_BIN" -R -s 'split("\n") | map(select(length > 0))')"
else
  CLOSING_FAILURES_JSON='[]'
fi

# ---------------------------------------------------------------------------
# metadata.json + summary.md
# ---------------------------------------------------------------------------

# shellcheck disable=SC2016 # single-quoted jq program; $vars are jq bindings
"$JQ_BIN" -n \
  --arg git_head "$("$GIT_BIN" -C "$REPO_ROOT" rev-parse HEAD)" \
  --arg rustc "$(rustc -V)" \
  --arg hyperfine "$("$HYPERFINE_BIN" --version)" \
  --arg loadavg_start "$LOADAVG_START" \
  --arg loadavg_end "$LOADAVG_END" \
  --arg xprotectd_cpu_start "$XPROTECTD_CPU_START" \
  --arg xprotectd_cpu_end "$XPROTECTD_CPU_END" \
  --arg idle_start "${IDLE_START:-0}" \
  --arg idle_end "${IDLE_END:-0}" \
  --arg top_proc_start "${TOP_PROC_START:-0}" \
  --arg top_proc_end "${TOP_PROC_END:-0}" \
  --arg idle_min "$IDLE_MIN" \
  --arg top_proc_max "$TOP_PROC_MAX" \
  --arg os_name "$(sw_vers -productName)" \
  --arg os_version "$(sw_vers -productVersion)" \
  --arg os_build "$(sw_vers -buildVersion)" \
  --arg cpu_brand "$(sysctl -n machdep.cpu.brand_string)" \
  --arg rustflags "$CURRENT_RUSTFLAGS" \
  --arg rustflags_frozen "$FROZEN_RUSTFLAGS" \
  --arg allow_flags "$ALLOW_FLAGS" \
  --arg binary "$SCAP_BIN" \
  --arg binary_size_bytes "$BIN_SIZE_BYTES" \
  --arg binary_sha256 "$BIN_SHA256" \
  --arg runs "$RUNS" \
  --arg warmup "$WARMUP" \
  --arg forced "$FORCE" \
  --arg foreign_scans "$FOREIGN_SCANS" \
  --arg out_real "$OUT_REAL" \
  --arg closing_checked "$CLOSING_CHECKED" \
  --argjson closing_failures "$CLOSING_FAILURES_JSON" \
  --argjson corpus_inventory "$INVENTORY_JSON" \
  '{
    git_head: $git_head,
    rustc: $rustc,
    hyperfine: $hyperfine,
    load_avg: { start: $loadavg_start, end: $loadavg_end },
    xprotectd_cpu_pct: { start: ($xprotectd_cpu_start | tonumber), end: ($xprotectd_cpu_end | tonumber) },
    cpu_idle_pct: { start: ($idle_start | tonumber), end: ($idle_end | tonumber) },
    top_proc_cpu_pct: { start: ($top_proc_start | tonumber), end: ($top_proc_end | tonumber) },
    gate: {
      deviation: "D-1: contention measured directly; load average recorded, not gated",
      idle_min_pct: ($idle_min | tonumber),
      top_proc_max_pct: ($top_proc_max | tonumber)
    },
    closing_gate: {
      checked: ($closing_checked == "1"),
      passed: (if $closing_checked == "1" then ($closing_failures | length) == 0 else null end),
      idle_pct: ($idle_end | tonumber),
      top_proc_cpu_pct: ($top_proc_end | tonumber),
      idle_min_pct: ($idle_min | tonumber),
      top_proc_max_pct: ($top_proc_max | tonumber),
      sampling: (if $closing_checked == "1" then "median of 3 top -l 2 samples 5 s apart, D-1a exclusions" else "single top -l 2 sample (forced run: recorded, not gated)" end),
      failures: $closing_failures,
      rule: "the closing sample is held to the same idle/busiest-process constants as the start gate; a failing group is DISCARDED - its rows, metadata and summary are kept and a DISCARDED marker is written, and the run exits 4 - so a discarded window is disclosed rather than dropped. SCAP_BENCH_FORCE=1 records the close without gating it, as it does the start."
    },
    os: { name: $os_name, version: $os_version, build: $os_build },
    cpu_brand: $cpu_brand,
    rustflags: $rustflags,
    rustflags_frozen: $rustflags_frozen,
    rustflags_match: ($rustflags == $rustflags_frozen),
    allow_flags: ($allow_flags == "1"),
    binary: $binary,
    binary_size_bytes: ($binary_size_bytes | tonumber),
    binary_sha256: $binary_sha256,
    runs: ($runs | tonumber),
    warmup: ($warmup | tonumber),
    forced: ($forced == "1"),
    foreign_hyperfine: {
      out_dir: $out_real,
      scans: ($foreign_scans | tonumber),
      detected: [],
      rule: "any hyperfine whose --export-json is outside out_dir is foreign; a detection aborts the run with exit 3 and writes OUT/CONTAMINATED, so a completed metadata.json always reports an empty list"
    },
    corpus_inventory: $corpus_inventory
  }' > "$OUT/metadata.json"

{
  printf '# Benchmark summary\n\n'
  printf 'Generated: %s\n\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf '| Row | Mean +/- sigma (ms) | Median (ms) | IQR q1-q3 (ms) | Min (ms) | User (ms) | Sys (ms) |\n'
  printf '| --- | --- | --- | --- | --- | --- | --- |\n'
  for row in "${RAN_ROWS[@]}"; do
    json="$OUT/$row.json"
    [[ -f "$json" ]] || continue
    # shellcheck disable=SC2016 # single-quoted jq program; $vars are jq bindings
    "$JQ_BIN" -r '
      def q(arr; p):
        (arr | length) as $n
        | (p * ($n - 1)) as $h
        | ($h | floor) as $lo
        | ($h | ceil) as $hi
        | if $lo == $hi then arr[$lo]
          else arr[$lo] + ($h - $lo) * (arr[$hi] - arr[$lo])
          end;
      .results[0] as $r
      | ($r.times | sort) as $t
      | [
          ($r.mean * 1000),
          ($r.stddev * 1000),
          ($r.median * 1000),
          (q($t; 0.25) * 1000),
          (q($t; 0.75) * 1000),
          ($r.min * 1000),
          ($r.user * 1000),
          ($r.system * 1000)
        ] | @tsv
    ' "$json" | while IFS=$'\t' read -r mean stddev median q1 q3 min user sys; do
      printf '| %s | %.3f +/- %.3f | %.3f | %.3f-%.3f | %.3f | %.3f | %.3f |\n' \
        "$row" "$mean" "$stddev" "$median" "$q1" "$q3" "$min" "$user" "$sys"
    done
  done
} > "$OUT/summary.md"

echo "bench-quiet.sh: wrote $OUT/metadata.json and $OUT/summary.md (${#RAN_ROWS[@]} rows)" >&2

if (( ${#CLOSING_FAILURES[@]} > 0 )); then
  {
    echo "bench-quiet.sh: closing gate failed - this group is DISCARDED:"
    printf '  - %s\n' "${CLOSING_FAILURES[@]}"
    echo "The group started quiet and ended contended, so its later rows were"
    echo "measured against a competitor. The run directory is kept: report the"
    echo "discard, do not report the rows."
  } >&2
  # Marker file, so a reader who finds this directory later cannot mistake it
  # for an admitted group even without reading metadata.json.
  {
    printf 'Discarded %s: closing gate failed.\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf '%s\n' "${CLOSING_FAILURES[@]}"
    printf 'Rows kept as evidence; they must not be reported as a measurement.\n'
  } >> "$OUT/DISCARDED"
  exit 4
fi
