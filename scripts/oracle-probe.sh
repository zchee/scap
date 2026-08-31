#!/usr/bin/env bash
set -uo pipefail
# errexit is intentionally NOT set: every fixture below must run even if an
# earlier one fails to construct on this filesystem (see case 9) or the
# oracle itself crashes (case 13b) — each case captures and records its own
# outcome instead of aborting the whole probe.

# scripts/oracle-probe.sh [outfile]
#
# W0.4 walker-semantics oracle probe (plan
# .omc/plans/2026-08-28-theoretical-limit-optimization.md, ADR-9). Builds one
# fresh fixture per case below with real `git init`, runs the real ghq binary
# and the current scap binary against it as GHQ_ROOT=/SCAP_ROOT=, captures
# stdout/stderr/exit for each, and writes a Markdown evidence table. This
# freezes ADR-9 rules (iii) (symlink/gitdir semantics), (iv) (`.git` entry
# semantics) and (vi) (root-stat semantics), and pins the expected oracle
# output for tests/list.rs:167 and tests/list.rs:553.
#
# Env:
#   GHQ_BINARY  required; path to the oracle ghq binary. Never falls back to
#               PATH (anti-vacuous rule, matches scripts/parity-corpus.sh).
#   SCAP_BIN    optional; default <repo>/target/release/scap, built with a
#               plain `cargo build --release` if missing.
#
# Every case is cleaned up (permissions restored, then removed) before the
# script exits, including on a failed/aborted run.
#
# Agreement column, and what the frozen record means. Until W5.4 the
# Agreement verdict compared stdout only, so a case where the two tools
# printed the same thing but exited differently was recorded as `Y`. The
# frozen W0.4 record (docs/benchmarks/2026-08-28-oracle-probe.md) was
# produced under that rule and is deliberately NOT re-run, so read two of its
# rows with that in mind:
#
#   - Row 13b (ENOTDIR root) is a genuine stdout-only `Y`: ghq exits 2 after
#     a nil-pointer panic and scap exits 0, and both print nothing. Under the
#     rule below it would read `N (stdout only: ghq exit 2, scap exit 0)`.
#     The row's own note already says this case is asserted directly rather
#     than against the oracle, so the record's conclusion is unaffected.
#   - Row 4 (ELOOP symlink loop) is the other row whose `Y` rested on the
#     stdout-only rule, and it is the one where a `timeout` kill of a hung
#     walk would have been indistinguishable from agreement. The record shows
#     exit 0 for both tools, so that row was in fact a true agreement; it
#     would still read `Y` under the rule below.
#
# No other row in the frozen record has divergent exit statuses.

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
outfile=${1:-"$repo_root/docs/benchmarks/2026-08-28-oracle-probe.md"}

if [[ -z "${GHQ_BINARY:-}" ]]; then
  echo "oracle-probe: GHQ_BINARY is unset; refusing to fall back to PATH" >&2
  exit 2
fi
if [[ ! -x "$GHQ_BINARY" ]]; then
  echo "oracle-probe: GHQ_BINARY=$GHQ_BINARY is not an executable file" >&2
  exit 2
fi

scap_bin=${SCAP_BIN:-"$repo_root/target/release/scap"}
if [[ ! -x "$scap_bin" ]]; then
  echo "oracle-probe: $scap_bin missing; building (cargo build --release)" >&2
  (cd "$repo_root" && cargo build --release) || {
    echo "oracle-probe: cargo build --release failed" >&2
    exit 2
  }
fi
if [[ ! -x "$scap_bin" ]]; then
  echo "oracle-probe: SCAP_BIN=$scap_bin still missing after build" >&2
  exit 2
fi

workroot=$(mktemp -d "${TMPDIR:-/tmp}/scap-oracle-probe.XXXXXX")
cleanup() {
  # Undo any chmod-000 fixtures before rm -rf, or the removal itself fails.
  chmod -R u+rwx "$workroot" 2>/dev/null || true
  rm -rf "$workroot"
}
trap cleanup EXIT

# Isolate git and identity so `git init` never touches the real user's
# config, and so ghq/scap never resolve against the real $HOME's
# scap.root/ghq.root (mirrors tests/list.rs's `isolated()` helper).
export NO_COLOR=1
export GIT_CONFIG_NOSYSTEM=1
export GIT_CONFIG_GLOBAL="$workroot/gitconfig-empty"
: >"$GIT_CONFIG_GLOBAL"
export GIT_AUTHOR_NAME=scap-oracle-probe
export GIT_AUTHOR_EMAIL=oracle-probe@example.invalid
export GIT_COMMITTER_NAME=scap-oracle-probe
export GIT_COMMITTER_EMAIL=oracle-probe@example.invalid
export HOME="$workroot/home"
mkdir -p "$HOME"

git_init() { git init -q -b main "$1" >/dev/null 2>&1; }
git_init_bare() { git init -q -b main --bare "$1" >/dev/null 2>&1; }

strip_ansi() {
  # ghq's colorine logger still emits a bare SGR-reset even under NO_COLOR=1
  # (observed live: "warning\x1b[0m <path>: Permission denied"); strip
  # ANSI/SGR escapes so captured text renders cleanly in a Markdown cell.
  printf '%s' "$1" | sed -E $'s/\x1b\\[[0-9;]*m//g'
}

md_escape() {
  local s
  s=$(strip_ansi "$1")
  s=${s//\\/\\\\}
  s=${s//|/\\|}
  s=${s//$'\n'/<br>}
  printf '%s' "$s"
}

# Truncate a captured stream to a table-friendly size; the ENOTDIR panic case
# (13b) needs only the "stack head", not ghq's full goroutine dump.
truncate_for_table() {
  local s=$1
  local head
  head=$(printf '%s' "$s" | head -c 800)
  if [[ ${#s} -gt ${#head} ]]; then
    head="${head}...<truncated>"
  fi
  md_escape "$head"
}

rows=()

# run_case <id> <description> <root> <adr9-rule>
#
# Runs `list` against $root with both binaries, capturing stdout/stderr/exit,
# and appends one Markdown row. Neither invocation is allowed to abort the
# script (errexit is off; each is timeout-guarded against the symlink-loop
# class of hang).
run_case() {
  local id=$1 desc=$2 root=$3 rule=$4
  local ghq_out ghq_err ghq_status scap_out scap_err scap_status agree

  ghq_out=$(GHQ_ROOT="$root" timeout 10 "$GHQ_BINARY" list 2>"$workroot/ghq.err")
  ghq_status=$?
  ghq_err=$(<"$workroot/ghq.err")

  scap_out=$(SCAP_ROOT="$root" timeout 10 "$scap_bin" list 2>"$workroot/scap.err")
  scap_status=$?
  scap_err=$(<"$workroot/scap.err")

  # Agreement is over the pair (stdout, exit status), not stdout alone: two
  # tools that both print nothing while one of them crashes have not agreed
  # about anything. A `timeout` kill (124, or 137 after a --kill-after) is
  # never agreement either, however similar the two empty stdouts look.
  agree=N
  if [[ $ghq_status -eq 124 || $ghq_status -eq 137 || $scap_status -eq 124 || $scap_status -eq 137 ]]; then
    agree="N (timeout kill: ghq exit $ghq_status, scap exit $scap_status)"
  elif [[ "$ghq_out" == "$scap_out" && "$ghq_status" -eq "$scap_status" ]]; then
    agree=Y
  elif [[ "$ghq_out" == "$scap_out" ]]; then
    agree="N (stdout only: ghq exit $ghq_status, scap exit $scap_status)"
  fi

  rows+=("| $id | $(md_escape "$desc") | $(truncate_for_table "$ghq_out") | $(truncate_for_table "$ghq_err") (exit $ghq_status) | $(truncate_for_table "$scap_out") | $(truncate_for_table "$scap_err") (exit $scap_status) | $rule | $agree |")
}

# run_case_unconstructible <id> <description> <reason> <adr9-rule>
#
# Records a case whose fixture could not be built on this filesystem, rather
# than silently skipping it.
run_case_unconstructible() {
  local id=$1 desc=$2 reason=$3 rule=$4
  rows+=("| $id | $(md_escape "$desc") | _n/a_ | $(md_escape "$reason") | _n/a_ | $(md_escape "$reason") | $rule | N/A (fixture unconstructible) |")
}

### Case 1 — symlink -> repo dir (expect ghq emits at the link's path)
c1="$workroot/case-01"
mkdir -p "$c1/root"
git_init "$c1/target-repo"
ln -s "$c1/target-repo" "$c1/root/link-to-repo"
run_case "1" "root/link-to-repo -> \$tmp/target-repo (git repo, outside root)" "$c1/root" "(iii)"

### Case 2 — symlink -> non-repo dir that contains repos (expect ghq emits nothing under it)
c2="$workroot/case-02"
mkdir -p "$c2/root" "$c2/target-dir/nested/repo"
git_init "$c2/target-dir/nested/repo"
ln -s "$c2/target-dir" "$c2/root/link-to-plain-dir"
run_case "2" "root/link-to-plain-dir -> \$tmp/target-dir (not a repo; target-dir/nested/repo is)" "$c2/root" "(iii)"

### Case 3 — dangling symlink
c3="$workroot/case-03"
mkdir -p "$c3/root"
ln -s "$c3/does-not-exist" "$c3/root/dangling"
run_case "3" "root/dangling -> \$tmp/does-not-exist (dangling)" "$c3/root" "(iii)"

### Case 4 — symlink loop (a -> b -> a)
c4="$workroot/case-04"
mkdir -p "$c4/root"
ln -s loop-b "$c4/root/loop-a"
ln -s loop-a "$c4/root/loop-b"
run_case "4" "root/loop-a -> loop-b -> loop-a (ELOOP)" "$c4/root" "(iii)"

### Case 5 — dangling .git symlink inside a dir (os.Stat follows -> not a repo)
c5="$workroot/case-05"
mkdir -p "$c5/root/dirwithdanglinggit"
ln -s "$c5/nowhere" "$c5/root/dirwithdanglinggit/.git"
run_case "5" "root/dirwithdanglinggit/.git -> \$tmp/nowhere (dangling)" "$c5/root" "(iv)"

### Case 6 — .git symlink pointing at a real gitdir (repo)
c6="$workroot/case-06"
mkdir -p "$c6/root/repo-via-git-symlink"
git_init "$c6/actual-repo"
ln -s "$c6/actual-repo/.git" "$c6/root/repo-via-git-symlink/.git"
run_case "6" "root/repo-via-git-symlink/.git -> \$tmp/actual-repo/.git (real gitdir)" "$c6/root" "(iv)"

### Case 7 — symlink -> bare x.git dir (link name does NOT itself end in .git)
c7="$workroot/case-07"
mkdir -p "$c7/root" "$c7/store"
git_init_bare "$c7/store/upstream.git"
ln -s "$c7/store/upstream.git" "$c7/root/link-to-bare"
run_case "7" "root/link-to-bare -> \$tmp/store/upstream.git (bare, link name lacks .git suffix)" "$c7/root" "(ii)+(iii)"

### Case 8 — symlinked ROOT (GHQ_ROOT/SCAP_ROOT itself is a symlink)
c8="$workroot/case-08"
mkdir -p "$c8/real-root"
git_init "$c8/real-root/github.com/a/x"
ln -s "$c8/real-root" "$c8/root-link"
run_case "8" "GHQ_ROOT/SCAP_ROOT = \$tmp/root-link -> \$tmp/real-root (root itself is a symlink)" "$c8/root-link" "(vi)"

### Case 9 — non-UTF-8 entry name
c9="$workroot/case-09"
mkdir -p "$c9/root"
bad_name=$(printf '\xff')
if mkdir "$c9/root/$bad_name" 2>"$workroot/mkdir9.err"; then
  git_init "$c9/root/$bad_name/repo"
  run_case "9" "root/<0xFF-byte-name>/repo (non-UTF-8 directory name)" "$c9/root" "byte-safety (list.rs:208 today drops non-UTF-8)"
else
  run_case_unconstructible "9" "root/<0xFF-byte-name>/repo (non-UTF-8 directory name)" \
    "mkdir rejected the name on this filesystem: $(<"$workroot/mkdir9.err")" \
    "byte-safety (list.rs:208 today drops non-UTF-8)"
fi

### Case 10 — root itself is a repo (expect ".")
c10="$workroot/case-10"
git_init "$c10/root"
run_case "10" "root is itself a git repository" "$c10/root" "(vi)"

### Case 11 — permission-denied subdirectory containing a repo
c11="$workroot/case-11"
mkdir -p "$c11/root/denied" "$c11/root/normal"
git_init "$c11/root/denied/repo"
git_init "$c11/root/normal/repo"
chmod 000 "$c11/root/denied"
run_case "11" "root/denied (chmod 000) contains denied/repo; root/normal/repo is a sibling" "$c11/root" "(v)"
chmod 700 "$c11/root/denied"

### Case 12 — non-existent root
c12="$workroot/case-12"
mkdir -p "$c12"
run_case "12" "GHQ_ROOT/SCAP_ROOT = \$tmp/does-not-exist (ENOENT)" "$c12/does-not-exist" "(vi)"

### Case 13a — unreadable root (chmod 000 the root itself)
c13a="$workroot/case-13a"
mkdir -p "$c13a/root"
chmod 000 "$c13a/root"
run_case "13a" "GHQ_ROOT/SCAP_ROOT = \$tmp/root, root itself chmod 000" "$c13a/root" "(vi) 2nd case"
chmod 700 "$c13a/root"

### Case 13b — non-ENOENT-failing root (a regular file where a directory is expected)
c13b="$workroot/case-13b"
mkdir -p "$c13b"
touch "$c13b/file"
run_case "13b" "GHQ_ROOT/SCAP_ROOT = \$tmp/file/dir (file is a regular file, not a dir: ENOTDIR)" "$c13b/file/dir" "(vi) 3rd case: ghq nil-deref panic, scap skip+warn (asserted directly, not vs oracle)"

### Case 14a — tests/list.rs:167 fixture (list_symlinked_repo_is_discovered_once)
c14a="$workroot/case-14a"
mkdir -p "$c14a/root"
git_init "$c14a/target/real/repo"
ln -s "$c14a/target" "$c14a/root/linked"
run_case "14a" "tests/list.rs:167 layout: root/linked -> \$tmp/target, target/real/repo is a git repo" "$c14a/root" "(iii); pins tests/list.rs:167 expectation"

### Case 14b — tests/list.rs:553 fixture (list_includes_symlinked_repository_target)
c14b="$workroot/case-14b"
git_init "$c14b/root/github.com/a/x"
ln -s "$c14b/root/github.com/a/x" "$c14b/root/mirror"
run_case "14b" "tests/list.rs:553 layout: root/github.com/a/x is a repo; root/mirror -> it (same root)" "$c14b/root" "(iii); pins tests/list.rs:553 expectation"

### Write the evidence table
mkdir -p "$(dirname "$outfile")"
{
  echo "# W0.4 oracle probe: walker-semantics evidence"
  echo
  echo "Generated by \`scripts/oracle-probe.sh\` against \`GHQ_BINARY=$GHQ_BINARY\`"
  echo "($("$GHQ_BINARY" --version 2>&1 | head -1)) and \`SCAP_BIN=$scap_bin\`."
  echo
  echo "Freezes ADR-9 rules (iii) symlink/gitdir semantics, (iv) \`.git\` entry"
  echo "semantics, (vi) root-stat semantics, and pins the oracle expectation"
  echo "for tests/list.rs:167 and tests/list.rs:553"
  echo "(.omc/plans/2026-08-28-theoretical-limit-optimization.md, W0.4)."
  echo
  echo "Long stdout/stderr captures are truncated to 800 characters for table"
  echo "readability (case 13b's ghq goroutine dump in particular is cut down"
  echo "to its stack head, per the task's \"capture the stack head\" rule)."
  echo "Newlines inside a cell are rendered as \`<br>\`; ANSI/SGR escape"
  echo "sequences are stripped (ghq's colorine logger emits a bare SGR-reset"
  echo "even under \`NO_COLOR=1\`, observed in case 11 and 13a)."
  echo
  echo "| Case | Fixture description | ghq stdout | ghq stderr (exit) | scap stdout | scap stderr (exit) | ADR-9 rule frozen | Agreement |"
  echo "| --- | --- | --- | --- | --- | --- | --- | --- |"
  for row in "${rows[@]}"; do
    echo "$row"
  done
} >"$outfile"

echo "oracle-probe: wrote $outfile" >&2
