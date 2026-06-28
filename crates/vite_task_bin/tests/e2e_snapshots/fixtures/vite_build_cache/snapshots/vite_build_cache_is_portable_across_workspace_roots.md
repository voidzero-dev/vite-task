# vite_build_cache_is_portable_across_workspace_roots

Two identical plain Vite workspaces run from different absolute roots. The test copies the origin root's default task cache directory into the clone root to simulate uploading and downloading cache data; the clone should hit that copied cache and restore build outputs.

## `cd portable_origin && vt run --cache build`

origin workspace root: cache miss populates its default cache

```
$ vite build
```

## `cd portable_origin && vtt stat-file dist/assets/index.js`

origin emitted the non-hashed build asset

```
dist/assets/index.js: exists
```

## `vtt cp -r portable_origin/node_modules/.vite/task-cache portable_clone/node_modules/.vite/task-cache`

copy-paste the cache directory to simulate upload/download

```
```

## `cd portable_clone && vt run --cache build`

clone workspace root: cache hit from the origin root

```
$ vite build ◉ cache hit, replaying

---
vt run: cache hit.
```

## `cd portable_clone && vtt stat-file dist/assets/index.js`

clone restored the non-hashed asset from the portable cache archive

```
dist/assets/index.js: exists
```
