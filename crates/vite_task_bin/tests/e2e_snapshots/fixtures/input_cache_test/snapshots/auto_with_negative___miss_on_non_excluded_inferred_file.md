# auto_with_negative___miss_on_non_excluded_inferred_file

With `auto: true` plus a negative glob, changes to inferred inputs outside
the excluded range should still invalidate the cache.

## `vt run auto-with-negative`

```
$ vtt print-file src/main.ts dist/output.js
export const main = 'initial';
// initial output
```

## `vtt replace-file-content src/main.ts initial modified`

```
```

## `vt run auto-with-negative`

```
$ vtt print-file src/main.ts dist/output.js ○ cache miss: 'src/main.ts' modified, executing
export const main = 'modified';
// initial output
```
