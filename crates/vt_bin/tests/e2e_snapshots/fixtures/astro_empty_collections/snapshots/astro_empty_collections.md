# astro_empty_collections

`astro build` (Astro 6) on a one-page site makes `.astro/collections` and `node_modules/.astro` and leaves them empty, writes `.astro/*.d.ts`, rewrites `dist/.prerender/*` and then removes the folder, stats `dist`, lists the project root, and Vite's dependency optimizer renames `node_modules/.vite/deps_temp_*` to `deps` and reads it back.

## `vt run build`

```
$ ASTRO_TELEMETRY_DISABLED=1 astro build --silent
```

## `vtt rm -r .astro dist node_modules/.vite/deps node_modules/.astro`

remove Astro's outputs and caches, as on a clean checkout (vp's own cache is in `node_modules/.vite/task-cache`)

```
```

## `vt run build`

several inputs are missing and vt names whichever it finds first, so the miss reason is redacted

```
$ ASTRO_TELEMETRY_DISABLED=1 astro build --silent ○ cache miss: <removed input>, executing
```

## `vtt stat-file .astro/collections`

```
.astro/collections: exists
```
