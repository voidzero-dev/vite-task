# fetch_envs_tracks_glob_match_set

Exercises `getEnvs(pattern, { tracked: true })`. The glob `PROBE_*` and its match-set snapshot enter the post-run fingerprint: later runs miss on changed, added, or removed matching envs, but hit when only non-matching envs differ.

## `PROBE_A=a PROBE_B=b vt run fetch-envs`

populate: first run captures {PROBE_A, PROBE_B} under the glob

```
$ node scripts/fetch_envs.mjs
PROBE_A=a
PROBE_B=b
```

## `PROBE_A=a PROBE_B=b vt run fetch-envs`

unchanged: same match-set -> cache hit

```
$ node scripts/fetch_envs.mjs ◉ cache hit, replaying
PROBE_A=a
PROBE_B=b

---
vt run: cache hit.
```

## `PROBE_A=changed PROBE_B=b vt run fetch-envs`

change: PROBE_A value differs -> cache miss

```
$ node scripts/fetch_envs.mjs ○ cache miss: env 'PROBE_A' changed, executing
PROBE_A=changed
PROBE_B=b
```

## `PROBE_A=changed PROBE_B=b PROBE_C=c vt run fetch-envs`

add: PROBE_C is new under the glob -> cache miss

```
$ node scripts/fetch_envs.mjs ○ cache miss: env 'PROBE_C' added, executing
PROBE_A=changed
PROBE_B=b
PROBE_C=c
```

## `PROBE_B=b PROBE_C=c vt run fetch-envs`

remove: PROBE_A dropped from the match-set -> cache miss

```
$ node scripts/fetch_envs.mjs ○ cache miss: env 'PROBE_A' removed, executing
PROBE_B=b
PROBE_C=c
```

## `PROBE_B=b PROBE_C=c UNRELATED=noise vt run fetch-envs`

non-matching noise: UNRELATED does not match PROBE_* -> cache hit

```
$ node scripts/fetch_envs.mjs ◉ cache hit, replaying
PROBE_B=b
PROBE_C=c

---
vt run: cache hit.
```
