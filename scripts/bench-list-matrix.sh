#!/usr/bin/env bash
set -euo pipefail

if [[ -n "${SCAP_BENCH_ROOTS:-}" ]]; then
  IFS=":" read -r -a ROOT_REAL <<< "${SCAP_BENCH_ROOTS}"
else
  ROOT_REAL=(
    "/Users/zchee/src"
    "/Users/zchee/go/src"
  )
fi
ROOT_REAL_CSV="$(printf '%s:' "${ROOT_REAL[@]}" | sed 's/:$//')"

shell_dq() {
  local value=$1
  value=${value//\\/\\\\}
  value=${value//\"/\\\"}
  value=${value//\$/\\$}
  value=${value//\`/\\\`}
  printf '"%s"' "$value"
}

quote_join() {
  local quoted=()
  local value
  for value in "$@"; do
    quoted+=("$(shell_dq "$value")")
  done
  printf '%s' "${quoted[*]}"
}

REPO_ROOT="$(git rev-parse --show-toplevel)"
SCAP_BIN="${SCAP_BIN:-$REPO_ROOT/target/release/scap}"
GHQ_BIN="${GHQ_BIN:-$(command -v ghq)}"
FD_BIN="${FD_BIN:-$(command -v fd)}"
FIND_BIN="${FIND_BIN:-$(command -v find)}"
HYPERFINE_BIN="${HYPERFINE_BIN:-$(command -v hyperfine)}"
DRY_RUN="${SCAP_BENCH_DRY_RUN:-0}"

ROOT_REAL_CSV_Q="$(shell_dq "$ROOT_REAL_CSV")"
ROOT_REAL_ARGS_Q="$(quote_join "${ROOT_REAL[@]}")"
SCAP_BIN_Q="$(shell_dq "$SCAP_BIN")"
GHQ_BIN_Q="$(shell_dq "$GHQ_BIN")"
FD_BIN_Q="$(shell_dq "$FD_BIN")"
FIND_BIN_Q="$(shell_dq "$FIND_BIN")"

SYNTH_HOSTS="${SCAP_BENCH_SYNTH_HOSTS:-5}"
SYNTH_USERS="${SCAP_BENCH_SYNTH_USERS:-4}"
SYNTH_REPOS="${SCAP_BENCH_SYNTH_REPOS:-10}"
SYNTH_NOISE="${SCAP_BENCH_SYNTH_NOISE:-120}"

ASSET_DIR="$REPO_ROOT/.omx/assets/scap-list-oss-fastest"
RUN_ID="${SCAP_BENCH_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)}"
RUN_DIR="$ASSET_DIR/$RUN_ID"
mkdir -p "$RUN_DIR/.commands" "$RUN_DIR/real" "$RUN_DIR/synthetic"
SYNTH_ROOT="$(mktemp -d "$RUN_DIR/synthetic-root-XXXXXX")"

cat > "$RUN_DIR/metadata.json" <<JSON
{
  "run_id": "$RUN_ID",
  "head": "$(git rev-parse HEAD)",
  "scap_bin": "$SCAP_BIN",
  "git_head_short": "$(git -C "$REPO_ROOT" rev-parse --short HEAD)",
  "rustc": "$(rustc -V)",
  "cargo": "$(cargo -V)",
  "ghq": "${GHQ_BIN:+$("$GHQ_BIN" --version | tr '\n' ' ' | sed 's/[[:space:]]\+/ /g')}",
  "fd": "${FD_BIN:+$("$FD_BIN" --version | head -n 1)}",
  "find": "${FIND_BIN:+$("$FIND_BIN" --version 2>/dev/null | head -n 1)}",
  "scap": "${SCAP_BIN:+$("$SCAP_BIN" --version 2>/dev/null | tr '\n' ' ' | sed 's/[[:space:]]\+/ /g')}",
  "benchmark": {
    "hyperfine": "$("$HYPERFINE_BIN" --version | head -n 1)",
    "runs": 20,
    "warmup": 5
  },
  "hardware": {
    "os": "$(sw_vers -productName) $(sw_vers -productVersion) (build $(sw_vers -buildVersion))",
    "kernel": "$(uname -a)",
    "cpu": "$(sysctl -n machdep.cpu.brand_string)"
  },
  "synthetic": {
    "hosts": "$SYNTH_HOSTS",
    "users": "$SYNTH_USERS",
    "repos": "$SYNTH_REPOS",
    "noise": "$SYNTH_NOISE"
  }
}
JSON

run_case() {
  local label=$1
  local cmd=$2
  local out_json=$3
  local cmd_file="$RUN_DIR/.commands/$label.sh"
  local log="/tmp/hyperfine-$label.log"

  printf '#!/usr/bin/env bash\nset -euo pipefail\n%s\n' "$cmd" > "$cmd_file"
  chmod +x "$cmd_file"

  if [[ "$DRY_RUN" == "1" || "$DRY_RUN" == "true" || "$DRY_RUN" == "TRUE" ]]; then
    cat > "$out_json" <<JSON
{
  "results": [
    {
      "command": "$label",
      "mean": 0.0,
      "stddev": 0.0,
      "median": 0.0,
      "user": 0.0,
      "system": 0.0,
      "min": 0.0,
      "max": 0.0,
      "times": [0.0],
      "exit_codes": [0]
    }
  ]
}
JSON
    return 0
  fi

  "$HYPERFINE_BIN" \
    --export-json "$out_json" \
    --warmup 5 \
    --runs 20 \
    --command-name "$label" \
    "$cmd_file" \
    >"$log" 2>&1 || {
      echo "hyperfine failed for $label. Last log at $log" >&2
      cat "$log" >&2
      return 1
    }

  mv "$log" "$RUN_DIR/$label.hyperfine.log"
}

run_case real_ghq_sort "SCAP_ROOT=$ROOT_REAL_CSV_Q $GHQ_BIN_Q list | LC_ALL=C sort" "$RUN_DIR/real/ghq-sort.json"
run_case real_ghq_devnull "SCAP_ROOT=$ROOT_REAL_CSV_Q $GHQ_BIN_Q list > /dev/null" "$RUN_DIR/real/ghq-devnull.json"
run_case real_scap_sort "SCAP_ROOT=$ROOT_REAL_CSV_Q $SCAP_BIN_Q list | LC_ALL=C sort" "$RUN_DIR/real/scap-sort.json"
run_case real_scap_devnull "SCAP_ROOT=$ROOT_REAL_CSV_Q $SCAP_BIN_Q list > /dev/null" "$RUN_DIR/real/scap-devnull.json"
run_case real_fd_raw_sort "$FD_BIN_Q --hidden --no-ignore --type d --glob '*.git' $ROOT_REAL_ARGS_Q | sed 's#/\\.git$##' | LC_ALL=C sort" "$RUN_DIR/real/fd-raw-sort.json"
run_case real_find_raw_sort "($FIND_BIN_Q $ROOT_REAL_ARGS_Q -type d -name .git -print 2>/dev/null || true) | sed 's#/\\.git$##' | LC_ALL=C sort" "$RUN_DIR/real/find-raw-sort.json"
run_case real_fd_raw_devnull "$FD_BIN_Q --hidden --no-ignore --type d --glob '*.git' $ROOT_REAL_ARGS_Q > /dev/null" "$RUN_DIR/real/fd-raw-devnull.json"
run_case real_find_raw_devnull "($FIND_BIN_Q $ROOT_REAL_ARGS_Q -type d -name .git -print 2>/dev/null || true) > /dev/null" "$RUN_DIR/real/find-raw-devnull.json"

printf 'Generating synthetic corpus at %s\n' "$SYNTH_ROOT"
for host in $(seq 1 "$SYNTH_HOSTS"); do
  for user in $(seq 1 "$SYNTH_USERS"); do
    for repo in $(seq 1 "$SYNTH_REPOS"); do
      mkdir -p "$SYNTH_ROOT/host-$host/user-$user/repo-$repo/.git"
    done
  done
done

for i in $(seq 1 "$SYNTH_NOISE"); do
  mkdir -p "$SYNTH_ROOT/noise/layer-$i/branch-$i/sub/$i"
done

mkdir -p "$SYNTH_ROOT/direct-root/.git"
mkdir -p "$SYNTH_ROOT/.hidden/.private/user-1/repo-hidden/.git"
mkdir -p "$SYNTH_ROOT/file-marker-target/.git"
mkdir -p "$SYNTH_ROOT/file-marker/user-1/repo-marker"
printf 'gitdir: %s/.git' "$SYNTH_ROOT/file-marker-target/.git" > "$SYNTH_ROOT/file-marker/user-1/repo-marker/.git"
mkdir -p "$SYNTH_ROOT/symlink-target/user-1/repo-real/.git"
mkdir -p "$SYNTH_ROOT/symlink"
ln -s "$SYNTH_ROOT/symlink-target" "$SYNTH_ROOT/symlink-alias"

SYNTH_ROOT_Q="$(shell_dq "$SYNTH_ROOT")"

run_case synthetic_ghq_sort "SCAP_ROOT=$SYNTH_ROOT_Q $GHQ_BIN_Q list | LC_ALL=C sort" "$RUN_DIR/synthetic/ghq-sort.json"
run_case synthetic_scap_sort "SCAP_ROOT=$SYNTH_ROOT_Q $SCAP_BIN_Q list | LC_ALL=C sort" "$RUN_DIR/synthetic/scap-sort.json"
run_case synthetic_fd_raw_sort "$FD_BIN_Q --hidden --no-ignore --type d --glob '*.git' $SYNTH_ROOT_Q | sed 's#/\\.git$##' | LC_ALL=C sort" "$RUN_DIR/synthetic/fd-raw-sort.json"
run_case synthetic_find_raw_sort "($FIND_BIN_Q $SYNTH_ROOT_Q -type d -name .git -print 2>/dev/null || true) | sed 's#/\\.git$##' | LC_ALL=C sort" "$RUN_DIR/synthetic/find-raw-sort.json"

cat > "$RUN_DIR/matrix-summary.md" <<MD
# scap list benchmark matrix (run $RUN_ID)

## Scope
- Real roots: ${ROOT_REAL[*]}
- Synthetic roots: $SYNTH_ROOT
- Command runner: hyperfine --warmup 5 --runs 20

## Notes
- Real-row commands use SCAP_ROOT=$ROOT_REAL_CSV_Q
- Synthetic corpus contains dense ghq-shaped layout, direct-root repos,
  hidden path, symlinked top-level alias, and .git file-marker structure.
- Synthetic corpus is removed automatically after execution; raw artifacts
  and JSON data remain in this run directory.

## Matrix artifacts
- Metadata: \`metadata.json\`
- Real corpus: \`real/*.json\`
- Synthetic corpus: \`synthetic/*.json\`
- Raw command logs: \`*.hyperfine.log\`
MD

rm -rf "$SYNTH_ROOT"

echo "Benchmark matrix artifacts written to $RUN_DIR"
