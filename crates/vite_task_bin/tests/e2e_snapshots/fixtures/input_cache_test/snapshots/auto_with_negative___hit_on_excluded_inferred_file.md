# auto_with_negative___hit_on_excluded_inferred_file

A negative glob should filter out inferred inputs — a `dist/**`-excluded file
that the command happens to read should not invalidate the cache.

## `vt run auto-with-negative`

```
$ vtt print-file src/main.ts dist/output.js
export const main = 'initial';
// initial output
```

## `vtt replace-file-content dist/output.js initial modified`

```
```

## `vt run auto-with-negative`

```
$ vtt print-file src/main.ts dist/output.js ◉ cache hit, replaying
export const main = 'initial';
// initial output

---
vt run: cache hit.
```
