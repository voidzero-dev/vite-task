# positive_globs_only___cache_hit_on_second_run

With only positive globs (`src/**/*.ts`) and no file changes, a second run
should be a cache hit.

## `vt run positive-globs-only`

```
$ vtt print-file src/main.ts
export const main = 'initial';
```

## `vt run positive-globs-only`

```
$ vtt print-file src/main.ts ◉ cache hit, replaying
export const main = 'initial';

---
vt run: cache hit.
```
