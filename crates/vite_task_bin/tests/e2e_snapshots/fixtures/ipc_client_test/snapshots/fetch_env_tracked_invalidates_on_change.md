# fetch_env_tracked_invalidates_on_change

Exercises `getEnv(name, { tracked: true })`. The env value becomes part of the post-run fingerprint: the same value still hits, and a different value misses with the env var named in the miss message.

## `PROBE_ENV=first vt run fetch-env`

first run captures PROBE_ENV=first in the post-run fingerprint

```
$ node scripts/fetch_env.mjs PROBE_ENV
served PROBE_ENV=first
process.env PROBE_ENV=(unset)
```

## `PROBE_ENV=first vt run fetch-env`

cache hit: PROBE_ENV unchanged

```
$ node scripts/fetch_env.mjs PROBE_ENV ◉ cache hit, replaying
served PROBE_ENV=first
process.env PROBE_ENV=(unset)

---
vt run: cache hit.
```

## `PROBE_ENV=second vt run fetch-env`

cache miss: tracked PROBE_ENV changed

```
$ node scripts/fetch_env.mjs PROBE_ENV ○ cache miss: env 'PROBE_ENV' changed, executing
served PROBE_ENV=second
process.env PROBE_ENV=(unset)
```
