# W3.5c — sampling profile of the `N` = 4 user-CPU delta

Read with the "W3.5c" section of `docs/benchmarks/2026-08-29-w35b.md`.

`harness/` is the throwaway crate the profile was taken on: it lives outside
the repository, depends on `scap` by path, and calls the shipped
`cli::dispatch` in a loop so a 140 ms walk becomes a process long enough for
`sample` to attach to. Built with the D-4 frozen `RUSTFLAGS` and the same
release profile as the shipped binary. **The binary it produces is not
committed, and the repository working tree was not modified to build it.**

- `n{1,4}-sample-header.txt` — the `sample` run headers (tool version, dates).
- `n{1,4}-top-of-stack.txt` — the flat leaf-frame sections, the profile proper.
  Full call graphs are 3.6 MB and are not committed; these are the part the
  analysis uses.
- `control-timings.txt` — the four-independent-processes control and the two
  single-process references. This is the load-bearing evidence: it needs no
  profiler and is not subject to the sampler's attribution limit.

Verdict recorded in the doc: **UNIFORM**. The attributed user-space total does
not move between one thread and four (8.72 vs 8.13 ms per walk), no frame
carries the delta, and four independent processes reproduce about a third of it
with nothing shared but the machine.
