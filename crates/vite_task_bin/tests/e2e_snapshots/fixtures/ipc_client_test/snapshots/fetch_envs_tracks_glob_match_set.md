# fetch_envs_tracks_glob_match_set

Exercises `getEnvs(pattern, { tracked: true })`. The glob `PROBE_*` and
its match-set snapshot enter the post-run fingerprint: later runs diff the
current match-set against what was stored and miss on add / remove / change,
but hit when only non-matching envs differ.

## `PROBE_A=a PROBE_B=b vt run fetch-envs`

populate: first run captures {PROBE_A, PROBE_B} under the glob

```
$ node scripts/fetch_envs.mjs
```

## `vtt print-file dist/out.txt`

the tool observed both matching envs

```
PROBE_A=a
PROBE_B=b
```

## `PROBE_A=a PROBE_B=b vt run fetch-envs`

unchanged: same match-set → cache hit

```
$ node scripts/fetch_envs.mjs ◉ cache hit, replaying

---
vt run: cache hit.
```

## `PROBE_A=changed PROBE_B=b vt run fetch-envs`

change: PROBE_A value differs → cache miss (TrackedEnvGlobChanged / changed)

```
$ node scripts/fetch_envs.mjs ○ cache miss: tracked env glob 'PROBE_*' changed, executing
```

## `PROBE_A=changed PROBE_B=b PROBE_C=c vt run fetch-envs`

add: PROBE_C is new under the glob → cache miss (added)

```
$ node scripts/fetch_envs.mjs ○ cache miss: tracked env glob 'PROBE_*' changed, executing
```

## `PROBE_B=b PROBE_C=c vt run fetch-envs`

remove: PROBE_A dropped from the match-set → cache miss (removed)

```
$ node scripts/fetch_envs.mjs ○ cache miss: tracked env glob 'PROBE_*' changed, executing
```

## `PROBE_B=b PROBE_C=c UNRELATED=noise vt run fetch-envs`

non-matching noise: UNRELATED doesn't match PROBE_* → cache hit

```
$ node scripts/fetch_envs.mjs ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt print-file dist/out.txt`

match-set unchanged from the previous successful run

```
PROBE_B=b
PROBE_C=c
```
