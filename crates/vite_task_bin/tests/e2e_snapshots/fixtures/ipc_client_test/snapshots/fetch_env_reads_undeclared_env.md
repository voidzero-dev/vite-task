# fetch_env_reads_undeclared_env

Exercises `getEnv(name)`: the tool asks the runner for an env var that is not declared in the task's `env` list. This verifies runner-served envs resolve from the unfiltered spawn env context while the child process env remains cache-filtered.

## `PROBE_ENV=served vt run fetch-env`

runner serves undeclared PROBE_ENV from the unfiltered env context

```
$ node scripts/fetch_env.mjs PROBE_ENV
served PROBE_ENV=served
process.env PROBE_ENV=(unset)
```
