# fetch_envs_tracked_query_ignores_untracked_names

Exercises `getEnvs(pattern, { tracked: true })` combined with `untrackedEnv`. Names matching `untrackedEnv` are excluded from the tracked match-set at record and validation time, so an ambient variable swept up by a broad query (a CI orchestration id, for example) cannot invalidate the cache. The runner still serves those variables to the tool; a hit replays the recorded output, per the untracked contract.

## `PROBE_A=a vt run fetch-envs-untracked-noise`

populate: the tracked match-set records {PROBE_A}; PROBE_NOISE_* is excluded

```
$ node scripts/fetch_envs.mjs
PROBE_A=a
```

## `PROBE_A=a PROBE_NOISE_ID=run-1 vt run fetch-envs-untracked-noise`

ambient noise matches the query but also untrackedEnv -> cache hit

```
$ node scripts/fetch_envs.mjs ◉ cache hit, replaying
PROBE_A=a

---
vt run: cache hit.
```

## `PROBE_A=a PROBE_NOISE_ID=run-2 vt run fetch-envs-untracked-noise`

noise value changes -> still a hit

```
$ node scripts/fetch_envs.mjs ◉ cache hit, replaying
PROBE_A=a

---
vt run: cache hit.
```

## `PROBE_A=changed PROBE_NOISE_ID=run-2 vt run fetch-envs-untracked-noise`

tracked match PROBE_A changes -> cache miss

```
$ node scripts/fetch_envs.mjs ○ cache miss: env 'PROBE_A' changed, executing
PROBE_A=changed
PROBE_NOISE_ID=run-2
```
