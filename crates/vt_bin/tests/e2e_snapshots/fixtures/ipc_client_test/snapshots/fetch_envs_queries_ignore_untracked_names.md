# fetch_envs_queries_ignore_untracked_names

Bulk glob and literal-prefix queries still serve explicitly ignored variables, but changing those values replays the cached result.

## `PROBE_A=a PROBE_NOISE_ID=run-1 ACTIONS_ORCHESTRATION_ID=run-1 vt run fetch-envs-untracked-noise`

populate: the glob serves both tracked values and untracked noise

```
$ node scripts/fetch_envs.mjs --glob "{PROBE_*,ACTIONS_ORCHESTRATION_ID}"
ACTIONS_ORCHESTRATION_ID=run-1
PROBE_A=a
PROBE_NOISE_ID=run-1
```

## `PROBE_A=a PROBE_NOISE_ID=run-2 ACTIONS_ORCHESTRATION_ID=run-2 vt run fetch-envs-untracked-noise`

cache hit: both explicitly ignored values changed

```
$ node scripts/fetch_envs.mjs --glob "{PROBE_*,ACTIONS_ORCHESTRATION_ID}" ◉ cache hit, replaying
ACTIONS_ORCHESTRATION_ID=run-1
PROBE_A=a
PROBE_NOISE_ID=run-1

---
vt run: cache hit.
```

## `PROBE_A=a PROBE_NOISE_ID=run-1 vt run fetch-envs-prefix-untracked-noise`

populate: the literal-prefix query serves its untracked match too

```
$ node scripts/fetch_envs.mjs --prefix
PROBE_A=a
PROBE_NOISE_ID=run-1
```

## `PROBE_A=a PROBE_NOISE_ID=run-2 vt run fetch-envs-prefix-untracked-noise`

cache hit: literal-prefix queries honor the same exclusion

```
$ node scripts/fetch_envs.mjs --prefix ◉ cache hit, replaying
PROBE_A=a
PROBE_NOISE_ID=run-1

---
vt run: cache hit.
```
