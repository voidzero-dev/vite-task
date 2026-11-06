# every_tracked_env_change_is_listed

A miss lists every tracked env and query match that changed, not just the first, along with the first changed file. The inline reason names the env changes together.

## `TRACKED_ENV=first PROBE_A=a vt run fetch-env-and-envs`

```
$ node scripts/fetch_env_and_envs.mjs
TRACKED_ENV=first
PROBE_A=a
first
```

## `vtt write-file envs_input.txt second`

```
```

## `TRACKED_ENV=second PROBE_A=a PROBE_B=b vt run -v fetch-env-and-envs`

TRACKED_ENV changed, PROBE_B was added, and the input file was modified.

```
$ node scripts/fetch_env_and_envs.mjs ○ cache miss: envs 'PROBE_B', 'TRACKED_ENV' changed, executing
TRACKED_ENV=second
PROBE_A=a
PROBE_B=b
second


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 0 cache hits • 1 cache miss
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] ipc-client-test#fetch-env-and-envs: $ node scripts/fetch_env_and_envs.mjs ✓
      → Cache miss: env 'PROBE_B' added
                    env 'TRACKED_ENV' changed
                    'envs_input.txt' modified
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
