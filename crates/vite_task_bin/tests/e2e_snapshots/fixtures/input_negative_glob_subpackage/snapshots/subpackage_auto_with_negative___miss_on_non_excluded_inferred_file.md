# subpackage_auto_with_negative___miss_on_non_excluded_inferred_file

In a subpackage with `auto: true` plus `!dist/**`, changes to inferred inputs
outside `dist/` should still invalidate the cache.

## `vt run sub-pkg#auto-with-negative`

```
~/packages/sub-pkg$ vtt print-file src/main.ts dist/output.js
export const main = 'initial';
// initial output
```

## `vtt replace-file-content packages/sub-pkg/src/main.ts initial modified`

```
```

## `vt run sub-pkg#auto-with-negative`

```
~/packages/sub-pkg$ vtt print-file src/main.ts dist/output.js ○ cache miss: 'packages/sub-pkg/src/main.ts' modified, executing
export const main = 'modified';
// initial output
```
