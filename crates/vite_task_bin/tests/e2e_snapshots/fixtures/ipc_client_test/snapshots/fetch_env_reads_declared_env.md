# fetch_env_reads_declared_env

Exercises `getEnv(name)`: the tool asks the runner for a declared env var and prints the served value. This verifies the round-trip IPC behavior before any runner-reported env is added to the cache fingerprint.

## `PROBE_ENV=served vt run fetch-env-declared`

runner serves PROBE_ENV from the spawned task env map

```
$ node scripts/fetch_env.mjs
PROBE_ENV=served
```
