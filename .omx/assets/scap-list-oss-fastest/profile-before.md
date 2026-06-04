# Profile evidence: `scap list` baseline (pre-edit)

Date: 2026-05-23 (UTC snapshot in this lane)
Repo: `/Users/zchee/rust/src/github.com/zchee/scap`
Scope: Task 2 (`debugger:profiling`) from `.omx/plans/2026-05-24-scap-list-oss-fastest-performance.md`

## Environment
- Host: `Darwin zchee-MacBook-Pro.local 25.5.0 Darwin Kernel Version 25.5.0... arm64 arm Darwin`
- Toolchain:
  - `rustc 1.95.0 (59807616e 2026-04-14)`
  - `cargo 1.95.0 (f2d3ce0bd 2026-03-21)`
- Commit: `14d27fa` was the pre-launch baseline in plan context; working tree in this run is on top of that history.
- Command root used for baselines:
  - `SCAP_ROOT="/Users/zchee/src:/Users/zchee/go/src"`

## Profile artifacts collected
All artifacts are written under
`.omx/assets/scap-list-oss-fastest/` in the worktree:

- `profile-before-compat-run1.time`
- `profile-before-compat-run2.time`
- `profile-before-compat-run3.time`
- `profile-before-devnull-run1.time`
- `profile-before-devnull-run2.time`
- `profile-before-devnull-run3.time`
- `profile-before-ghq-devnull-run1.time`
- `profile-before-ghq-devnull-run2.time`
- `profile-before-ghq-devnull-run3.time`

## Commands executed
```bash
# warm/compat output mode
SCAP_ROOT="/Users/zchee/src:/Users/zchee/go/src" /usr/bin/time -l ./target/release/scap list >/tmp/profile-list-output-1.txt 2>.omx/assets/scap-list-oss-fastest/profile-before-compat-run1.time
SCAP_ROOT="/Users/zchee/src:/Users/zchee/go/src" /usr/bin/time -l ./target/release/scap list >/tmp/profile-list-output-2.txt 2>.omx/assets/scap-list-oss-fastest/profile-before-compat-run2.time
SCAP_ROOT="/Users/zchee/src:/Users/zchee/go/src" /usr/bin/time -l ./target/release/scap list >/tmp/profile-list-output-3.txt 2>.omx/assets/scap-list-oss-fastest/profile-before-compat-run3.time

# warm/scan-to-devnull mode
SCAP_ROOT="/Users/zchee/src:/Users/zchee/go/src" /usr/bin/time -l ./target/release/scap list >/dev/null 2>.omx/assets/scap-list-oss-fastest/profile-before-devnull-run1.time
SCAP_ROOT="/Users/zchee/src:/Users/zchee/go/src" /usr/bin/time -l ./target/release/scap list >/dev/null 2>.omx/assets/scap-list-oss-fastest/profile-before-devnull-run2.time
SCAP_ROOT="/Users/zchee/src:/Users/zchee/go/src" /usr/bin/time -l ./target/release/scap list >/dev/null 2>.omx/assets/scap-list-oss-fastest/profile-before-devnull-run3.time

# comparator baseline
SCAP_ROOT="/Users/zchee/src:/Users/zchee/go/src" /usr/bin/time -l ghq list >/dev/null 2>.omx/assets/scap-list-oss-fastest/profile-before-ghq-devnull-run1.time
...
```

## Raw summary
### scap list compatibility output (to stdout file)
- run1: `0.35 real 0.12 user 0.68 sys`
- run2: `0.48 real 0.13 user 0.76 sys`
- run3: `0.40 real 0.12 user 0.58 sys`

### scap list to `/dev/null`
- run1: `0.26 real 0.14 user 0.72 sys`
- run2: `0.29 real 0.13 user 0.83 sys`
- run3: `0.25 real 0.13 user 0.73 sys`

### ghq devnull (3 runs)
- run1: `1.58 real 0.45 user 3.19 sys`
- run2: `2.49 real 0.47 user 3.25 sys`
- run3: `2.09 real 0.49 user 4.11 sys`

### Resource-level indicators
Across `scap list` baseline runs:
- `maximum resident set size` was typically 12.8–16.3 MB (`~12–16 MB`).
- `page faults` remained low (4 in most runs).
- `block input/output operations` were zero in sampled runs.
- CPU retire counts are on the order of `4.7e9–6.6e9` instructions for these runs.

## Interpretation / top bucket hypothesis
1. **Output/formatting overhead is meaningful**: compatibility mode is consistently `~0.10–0.23s` slower than devnull mode, indicating output emission + formatting path is a meaningful share at real-root scale.
2. **Traversal/syscall and sort costs remain dominant**: user+sys time is high relative to wall clock, with `sys` a substantial component.
3. **Scap remains materially faster than ghq devnull baseline** on this real-root corpus in this environment.

## Availability checks
Attempted to collect kernel-level syscall I/O profile, but:
- `dtrace` requires additional privileges in this environment.
- `fs_usage` requires root privileges here.

No additional low-level profiler was available without privilege escalation.

## Recommended follow-up (next lane)
- For `jwalk-micro-opts`, prioritize stdout path buffering and allocation-lazy paths before deeper traversal changes.
- Keep this file plus new JSON/bench artifacts side-by-side to ensure comparability.
