> **W3.5b, clean.** Drift-control sandwich, arm 2 of 2: design 2, binary
> sha256 `bd972564…`. NOT LANDED. Design 2 is **design 1 plus** `in_flight` on
> its own cache line and one atomic RMW per job instead of two — it is not an
> independent arm. Paired with `20260829T144300Z-w35b-s1before`.

# Benchmark summary

Generated: 2026-08-29T14:44:25Z

| Row | Mean +/- sigma (ms) | Median (ms) | IQR q1-q3 (ms) | Min (ms) | User (ms) | Sys (ms) |
| --- | --- | --- | --- | --- | --- | --- |
| list_ab_t1 | 341.232 +/- 13.212 | 338.887 | 329.248-352.518 | 327.439 | 26.244 | 313.655 |
| list_ab_t4 | 133.684 +/- 7.009 | 132.621 | 127.864-138.184 | 123.509 | 37.213 | 455.781 |
| list_ab_t8 | 128.190 +/- 7.279 | 126.501 | 123.411-129.240 | 121.353 | 51.767 | 847.216 |
