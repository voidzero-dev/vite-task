# fetch_env_tracks_with_explicit_inputs

Runner-aware env tracking must not depend on fspy auto-input inference. This task uses `input: []`, but `getEnv(name, { tracked: true })` still records the served value and later env changes miss.

## `PROBE_ENV=first vt run fetch-env-explicit-input`

first run captures PROBE_ENV even though input auto-inference is disabled

```
$ node scripts/fetch_env.mjs PROBE_ENV
served PROBE_ENV=first
process.env PROBE_ENV=(unset)
```

## `PROBE_ENV=first vt run fetch-env-explicit-input`

cache hit: explicit inputs are unchanged and PROBE_ENV is unchanged

```
$ node scripts/fetch_env.mjs PROBE_ENV ◉ cache hit, replaying
served PROBE_ENV=first
process.env PROBE_ENV=(unset)

---
vt run: cache hit.
```

## `PROBE_ENV=second vt run fetch-env-explicit-input`

cache miss: tracked env changed despite input auto-inference being disabled

```
$ node scripts/fetch_env.mjs PROBE_ENV ○ cache miss: env 'PROBE_ENV' changed, executing
served PROBE_ENV=second
process.env PROBE_ENV=(unset)
```
