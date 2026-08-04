# fetch_env_untracked_does_not_invalidate

Exercises `getEnv(name, { tracked: false })`. The runner still serves the env value, but the value does not enter the post-run fingerprint, so changing it later replays the cached output.

## `PROBE_ENV=first vt run fetch-env-untracked`

first run serves PROBE_ENV without tracking it

```
$ node scripts/fetch_env.mjs --untracked PROBE_ENV
served PROBE_ENV=first
process.env PROBE_ENV=(unset)
```

## `PROBE_ENV=second vt run fetch-env-untracked`

cache hit: PROBE_ENV changed but was requested with tracked: false

```
$ node scripts/fetch_env.mjs --untracked PROBE_ENV ◉ cache hit, replaying
served PROBE_ENV=first
process.env PROBE_ENV=(unset)

---
vt run: cache hit.
```
