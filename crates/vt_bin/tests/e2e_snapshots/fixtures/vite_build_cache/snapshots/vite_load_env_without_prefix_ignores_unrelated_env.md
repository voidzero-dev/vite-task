# vite_load_env_without_prefix_ignores_unrelated_env

A config calling `loadEnv(mode, dir, '')` makes Vite send `getEnvs({ prefix: '' })`, a query that matches every env name. The runner answers it with nothing and records nothing, so `loadEnv` returns the task's own env from `process.env`: `APP_LABEL`, which the task declares, but not `UNRELATED`. A change to `UNRELATED` doesn't make the next run miss.

## `APP_LABEL=alpha UNRELATED=one vt run --cache build-load-all-env`

first run: loadEnv sees the declared APP_LABEL only

```
$ vite build --config vite.load-all-env.config.js
APP_LABEL=alpha UNRELATED=(unset)
```

## `APP_LABEL=alpha UNRELATED=two vt run --cache build-load-all-env`

cache hit: UNRELATED changed, but it isn't in the cache key

```
$ vite build --config vite.load-all-env.config.js ◉ cache hit, replaying
APP_LABEL=alpha UNRELATED=(unset)

---
vt run: cache hit.
```

## `APP_LABEL=bravo UNRELATED=two vt run --cache build-load-all-env`

cache miss: the declared APP_LABEL changed

```
$ vite build --config vite.load-all-env.config.js ○ cache miss: env 'APP_LABEL' changed, executing
APP_LABEL=bravo UNRELATED=(unset)
```
