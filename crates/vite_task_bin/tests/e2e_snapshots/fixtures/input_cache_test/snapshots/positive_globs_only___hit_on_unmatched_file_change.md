# positive_globs_only___hit_on_unmatched_file_change

Modifying a file outside the positive glob (e.g. under `test/`) should not
invalidate the cache.

## `vt run positive-globs-only`

```
$ vtt print-file src/main.ts
export const main = 'initial';
```

## `vtt replace-file-content test/main.test.ts outside modified`

```
```

## `vt run positive-globs-only`

```
$ vtt print-file src/main.ts ◉ cache hit, replaying
export const main = 'initial';

---
vt run: cache hit.
```
