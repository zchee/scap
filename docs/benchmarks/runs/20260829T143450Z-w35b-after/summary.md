> **W3.5b, DEGRADED — cite with the caveat.** Paired with
> `20260829T143341Z-w35b-before`. The group started at 87.60 % idle and ended
> at **74.20 %**; `list_aprime` reads 18.361 against HEAD's own 15.774 in the
> preceding group and a frozen reference of 12.245, i.e. the corpus was
> measuring the host. Arm: design 1, binary sha256 `177d027b…`. NOT LANDED.

# Benchmark summary

Generated: 2026-08-29T14:35:49Z

| Row | Mean +/- sigma (ms) | Median (ms) | IQR q1-q3 (ms) | Min (ms) | User (ms) | Sys (ms) |
| --- | --- | --- | --- | --- | --- | --- |
| list_ab_t1 | 385.067 +/- 46.176 | 369.684 | 341.726-429.598 | 328.395 | 26.791 | 356.437 |
| list_ab_t2 | 253.137 +/- 10.753 | 254.501 | 248.336-257.336 | 232.297 | 32.923 | 458.303 |
| list_ab_t4 | 159.052 +/- 7.270 | 157.382 | 153.891-161.492 | 150.688 | 38.810 | 545.264 |
| list_ab_t8 | 140.369 +/- 3.738 | 140.118 | 138.752-142.513 | 132.602 | 54.031 | 931.145 |
| list_aprime | 18.774 +/- 2.144 | 18.361 | 17.594-18.860 | 16.757 | 4.704 | 33.514 |
| list_a | 141.886 +/- 5.635 | 142.186 | 136.426-146.327 | 132.079 | 31.172 | 470.618 |
