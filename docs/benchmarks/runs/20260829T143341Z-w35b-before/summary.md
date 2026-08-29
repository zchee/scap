> **W3.5b, DEGRADED — cite with the caveat.** First before/after pair. The
> group started at 86.68 % idle and ended at 81.63 % with a 140 % busiest
> process; its last two rows (`list_aprime`, `list_a`) took the worst part of
> the window, so their apparent regressions are host drift, not the change.
> Arm: HEAD `d626460`, binary sha256 `86dd2835…`. The clean replacement is
> `20260829T144300Z-w35b-s1before`.

# Benchmark summary

Generated: 2026-08-29T14:34:39Z

| Row | Mean +/- sigma (ms) | Median (ms) | IQR q1-q3 (ms) | Min (ms) | User (ms) | Sys (ms) |
| --- | --- | --- | --- | --- | --- | --- |
| list_ab_t1 | 343.583 +/- 11.018 | 341.623 | 335.867-348.028 | 327.448 | 26.243 | 315.981 |
| list_ab_t2 | 250.056 +/- 10.728 | 248.448 | 242.659-257.368 | 234.472 | 32.890 | 452.521 |
| list_ab_t4 | 157.463 +/- 5.268 | 157.892 | 153.670-160.946 | 146.813 | 38.568 | 544.409 |
| list_ab_t8 | 141.385 +/- 2.427 | 141.068 | 139.614-142.356 | 138.247 | 53.554 | 957.240 |
| list_aprime | 15.939 +/- 1.221 | 15.774 | 15.096-16.678 | 14.082 | 4.900 | 30.430 |
| list_a | 134.729 +/- 5.732 | 134.944 | 130.021-137.296 | 125.791 | 33.822 | 467.526 |
