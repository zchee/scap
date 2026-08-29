> **W3.5b, clean — the decisive group.** Five binaries at `N` = 4 plus two at
> `N` = 1, all inside one D-1a-clean window, so no two rows are separated by
> host drift. **`metadata.json` records only `SCAP_BIN` (the HEAD binary,
> sha256 `86dd2835…`); the rows measure five different binaries**, named by
> the row suffix and pinned here:
>
> | suffix | binary | sha256 | composition |
> | --- | --- | --- | --- |
> | `_head` | `scap-before` | `86dd2835…` | HEAD `d626460`, unchanged |
> | `_d1` | `scap-after` | `177d027b…` | design 1 (idle/steal path) |
> | `_d2` | `scap-d2` | `bd972564…` | **design 1 plus** isolated `in_flight` + one RMW per job |
> | `_d3` | `scap-d3` | `cdbe0318…` | HEAD plus isolated `live_fds` (`pool.rs` untouched) |
> | `_d4` | `scap-d4` | `58580bea…` | designs 1, 2 and 3 together |
>
> The sources are `design{1,2,3,4}.patch` in this directory, each applying
> cleanly to `d626460`; `counters.patch` is the instrumented build the
> execution counts were read from. None of them landed.

# Benchmark summary

Generated: 2026-08-29T14:51:27Z

| Row | Mean +/- sigma (ms) | Median (ms) | IQR q1-q3 (ms) | Min (ms) | User (ms) | Sys (ms) |
| --- | --- | --- | --- | --- | --- | --- |
| t4_head | 140.136 +/- 7.136 | 138.914 | 135.423-145.105 | 129.857 | 38.055 | 481.563 |
| t4_d1 | 151.024 +/- 28.956 | 143.079 | 137.421-148.303 | 128.612 | 39.384 | 500.823 |
| t4_d2 | 137.222 +/- 9.317 | 135.951 | 130.919-142.460 | 119.726 | 37.316 | 469.174 |
| t4_d3 | 148.304 +/- 12.538 | 145.125 | 139.980-155.053 | 131.700 | 39.770 | 510.750 |
| t4_d4 | 141.524 +/- 6.745 | 138.896 | 136.966-145.063 | 132.053 | 38.204 | 483.875 |
| t1_head | 358.924 +/- 16.856 | 353.941 | 346.836-372.591 | 335.114 | 26.841 | 330.786 |
| t1_d4 | 354.510 +/- 17.794 | 352.622 | 343.465-358.638 | 329.523 | 26.691 | 326.509 |
