# fetch_env_reads_honor_explicit_untracked_config

Explicit untrackedEnv applies equally to getEnv and an exact-name getEnvs query.

## `PROBE_ENV=first vt run fetch-env-explicitly-untracked`

populate: getEnv serves the explicitly ignored value

```
$ node scripts/fetch_env.mjs PROBE_ENV
served PROBE_ENV=first
process.env PROBE_ENV=first
```

## `PROBE_ENV=second vt run fetch-env-explicitly-untracked`

cache hit: explicit untrackedEnv suppresses the scalar dependency

```
$ node scripts/fetch_env.mjs PROBE_ENV ◉ cache hit, replaying
served PROBE_ENV=first
process.env PROBE_ENV=first

---
vt run: cache hit.
```

## `PROBE_ENV=first vt run fetch-envs-exact-explicitly-untracked`

populate: getEnvs serves the same exact-name query

```
$ node scripts/fetch_envs.mjs --glob PROBE_ENV
PROBE_ENV=first
```

## `PROBE_ENV=second vt run fetch-envs-exact-explicitly-untracked`

cache hit: the bulk query honors the same explicit policy

```
$ node scripts/fetch_envs.mjs --glob PROBE_ENV ◉ cache hit, replaying
PROBE_ENV=first

---
vt run: cache hit.
```
