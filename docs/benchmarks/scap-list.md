# `scap list` benchmark snapshot

Generated: 2026-05-24T04:58:14Z

## Scope

- Command target: `list` traversal (full-path output)
- Dataset path: `.omx/assets/scap-list-oss-fastest`
- Root fixture: 500 synthetic repositories (`github.com`, `gitlab.com`, `bitbucket.org`)
- Host: `Darwin arm64` at benchmark time

## Source artifacts

- `.omx/assets/scap-list-oss-fastest/dataset.toml`
- `.omx/assets/scap-list-oss-fastest/run.log`
- `.omx/assets/scap-list-oss-fastest/time-baseline.csv`
- `.omx/assets/scap-list-oss-fastest/time-baseline-raw.tsv`
- `.omx/assets/scap-list-oss-fastest/run-summary.json`

## Commands used

```bash
cargo build --release
time GHQ_ROOT="$BENCH_ROOT" target/release/scap list --full-path >/tmp/scap.list.out
# and
GHQ_ROOT="$BENCH_ROOT" ghq list --full-path >/tmp/ghq.list.out
```

`BENCH_ROOT` was set to the synthetic fixture root generated in
`.omx/assets/scap-list-oss-fastest/dataset.toml`.

## Parsed stats (real time, seconds)

- scap samples: 8
- ghq samples: 8
- scap: mean 0.0238s, median 0.0200s
- ghq: mean 0.0387s, median 0.0400s
- ratio (scap/ghq): mean 0.6125, median 0.5833

Interpretation: `scap list` shows lower median runtime than `ghq list` on this
synthetic fixture under current Rust implementation.
