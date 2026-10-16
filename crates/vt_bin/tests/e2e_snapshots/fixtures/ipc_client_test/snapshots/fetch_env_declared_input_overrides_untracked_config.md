# fetch_env_declared_input_overrides_untracked_config

A declared cache.env input still invalidates when the same name is explicitly listed in untrackedEnv.

## `PROBE_ENV=first vt run fetch-env-declared-and-untracked`

populate: cache.env explicitly fingerprints the value

```
$ node scripts/fetch_env.mjs PROBE_ENV
served PROBE_ENV=first
process.env PROBE_ENV=first
```

## `PROBE_ENV=second vt run fetch-env-declared-and-untracked`

cache miss: declared cache.env takes precedence over untrackedEnv

```
$ node scripts/fetch_env.mjs PROBE_ENV ○ cache miss: env 'PROBE_ENV' changed, executing
served PROBE_ENV=second
process.env PROBE_ENV=second
```
