# fetch_envs_prefix_reads_match_set

Exercises `getEnvs({ prefix: "PROBE_" })`: the tool asks the runner for every env whose name starts with `PROBE_` and prints the served match set.

## `PROBE_A=a PROBE_B=b PROBEX=not-a-prefix-match UNRELATED=noise vt run fetch-envs-prefix`

runner serves only envs with the literal PROBE_ prefix

```
$ node scripts/fetch_envs.mjs --prefix
PROBE_A=a
PROBE_B=b
```
