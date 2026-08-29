#!/usr/bin/env bash
# ADR-8 syscall-budget probe: how many filesystem calls one scap invocation
# costs, from a given working directory.
#
# Builds scripts/syscall-count.c into a temporary dyld interposing library,
# runs the command under it, and prints the per-call counts and a TOTAL with
# the empty-binary baseline subtracted -- dyld's own startup work is included
# in the raw figure and is not attributable to scap.
#
# Root-free by design: `fs_usage -w -f filesys` and `ktrace trace` both
# require root, and the reference machine's `sudo` is interactive-only. The
# figure is a lower bound: only the nine symbols syscall-count.c interposes
# are seen, so `opendir`/`readdir`, `getattrlist` and `realpath`'s internal
# `lstat` chain are not counted.
#
# Usage:
#   scripts/syscall-count.sh [-C <cwd>] [--] <command> [args...]
#   scripts/syscall-count.sh -C / target/release/scap root
#
# Env:
#   EMPTY_BIN  optional; the empty `fn main` binary used as the baseline.
#              Default <repo>/.omc/spikes/w00-empty/target/release/empty.
#              When it is absent the baseline is reported as 0 and the raw
#              total is printed instead of a net one.
#   TRACE      set to a path to also write one `<call> <path>` line per call.
#
# Exit codes: 0 measured; 2 usage error or the library would not build.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat >&2 <<'EOF'
Usage: scripts/syscall-count.sh [-C <cwd>] [--] <command> [args...]
EOF
}

run_cwd=$PWD
while (( $# > 0 )); do
  case "$1" in
    -C)
      shift
      [[ $# -gt 0 ]] || { usage; exit 2; }
      run_cwd=$1
      shift
      ;;
    --)
      shift
      break
      ;;
    -*)
      usage
      exit 2
      ;;
    *)
      break
      ;;
  esac
done

if (( $# == 0 )); then
  usage
  exit 2
fi

EMPTY_BIN="${EMPTY_BIN:-$REPO_ROOT/.omc/spikes/w00-empty/target/release/empty}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

lib="$work/syscall-count.dylib"
if ! cc -dynamiclib -O2 -o "$lib" "$REPO_ROOT/scripts/syscall-count.c" 2>"$work/cc.err"; then
  echo "syscall-count: failed to build the interposing library" >&2
  sed 's/^/      /' "$work/cc.err" >&2
  exit 2
fi

# Print the summary line one measured command produces, or nothing.
measure() {
  local out=$1 cwd=$2
  shift 2
  rm -f "$out"
  local trace_env=()
  if [[ -n "${TRACE:-}" ]]; then
    trace_env=(SYSCALL_COUNT_TRACE="$TRACE")
  fi
  local status=0
  (
    cd "$cwd" || exit 1
    env "${trace_env[@]}" SYSCALL_COUNT_OUT="$out" DYLD_INSERT_LIBRARIES="$lib" "$@" \
      >/dev/null 2>&1
  ) || status=$?
  if (( status != 0 )); then
    echo "syscall-count: '$*' exited $status; the counts below may be from a partial run" >&2
  fi
  cat "$out" 2>/dev/null || true
}

baseline_total=0
if [[ -x "$EMPTY_BIN" ]]; then
  baseline_line="$(measure "$work/baseline.txt" / "$EMPTY_BIN")"
  if [[ "$baseline_line" =~ TOTAL=([0-9]+) ]]; then
    baseline_total="${BASH_REMATCH[1]}"
  fi
else
  echo "syscall-count: no empty-binary baseline at $EMPTY_BIN; reporting the raw total" >&2
fi

line="$(measure "$work/run.txt" "$run_cwd" "$@")"
if [[ -z "$line" ]]; then
  echo "syscall-count: the command produced no counts (did it run?)" >&2
  exit 2
fi

total=0
if [[ "$line" =~ TOTAL=([0-9]+) ]]; then
  total="${BASH_REMATCH[1]}"
fi

echo "cwd:      $run_cwd"
echo "command:  $*"
echo "raw:      $line"
echo "baseline: $baseline_total (empty binary, same library)"
echo "net:      $(( total - baseline_total ))"
