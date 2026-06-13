# fetch_envs_reads_match_set

Exercises `getEnvs(pattern)`: the tool asks the runner for every env matching `PROBE_*` and prints the served match set. This verifies the bulk env IPC round trip before match sets are added to cache fingerprints.

## `PROBE_A=a PROBE_B=b UNRELATED=noise vt run fetch-envs`

runner serves only envs matching PROBE_*

```
$ node scripts/fetch_envs.mjs
PROBE_A=a
PROBE_B=b
```
