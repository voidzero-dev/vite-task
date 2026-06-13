# vite_node_env_change_invalidates_cache

`NODE_ENV` enters the build's cache fingerprint via Vite's `getEnv('NODE_ENV')` call in `resolveConfig`. Same value → cache hit; different value → cache miss with `envs changed`.

## `NODE_ENV=production vt run --cache build`

first run: NODE_ENV=production

```
$ vite build
```

## `NODE_ENV=production vt run --cache build`

cache hit: NODE_ENV unchanged

```
$ vite build ◉ cache hit, replaying

---
vt run: cache hit.
```

## `NODE_ENV=development vt run --cache build`

cache miss: envs changed (NODE_ENV changed)

```
$ vite build ○ cache miss: env 'NODE_ENV' changed, executing
```
