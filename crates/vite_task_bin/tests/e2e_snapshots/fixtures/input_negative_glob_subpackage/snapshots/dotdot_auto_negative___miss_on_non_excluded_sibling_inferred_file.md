# dotdot_auto_negative___miss_on_non_excluded_sibling_inferred_file

Under `auto: true` plus a `../` negative glob, inferred reads from sibling
files outside the exclusion should still invalidate the cache.

## `vt run sub-pkg#dotdot-auto-negative`

```
~/packages/sub-pkg$ vtt print-file ../shared/src/utils.ts ../shared/dist/output.js
export const shared = 'initial';
// initial output
```

## `vtt replace-file-content packages/shared/src/utils.ts initial modified`

```
```

## `vt run sub-pkg#dotdot-auto-negative`

```
~/packages/sub-pkg$ vtt print-file ../shared/src/utils.ts ../shared/dist/output.js ○ cache miss: 'packages/shared/src/utils.ts' modified, executing
export const shared = 'modified';
// initial output
```
