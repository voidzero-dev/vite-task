# fetch_env_tracked_invalidates_on_change

Exercises `getEnv(name, { tracked: true })`. The env value becomes part
of the post-run fingerprint: the same value still hits, a different value
misses with `tracked env 'PROBE_ENV' changed`.

## `PROBE_ENV=first vt run fetch-env`

first run captures PROBE_ENV=first in the fingerprint

```
$ node scripts/fetch_env.mjs
```

## `PROBE_ENV=first vt run fetch-env`

cache hit: PROBE_ENV unchanged

```
$ node scripts/fetch_env.mjs ◉ cache hit, replaying

---
vt run: cache hit.
```

## `PROBE_ENV=second vt run fetch-env`

cache miss: tracked env changed

```
$ node scripts/fetch_env.mjs ○ cache miss: tracked env 'PROBE_ENV' changed, executing
```
