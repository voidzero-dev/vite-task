# dotdot_positive_negative___miss_on_non_excluded_sibling_file

With `../shared/**` plus `!../shared/dist/**`, modifying a sibling file
outside the exclusion should invalidate the cache.

## `vt run sub-pkg#dotdot-positive-negative`

```
~/packages/sub-pkg$ vtt print-file ../shared/src/utils.ts ../shared/dist/output.js
export const shared = 'initial';
// initial output
```

## `vtt replace-file-content packages/shared/src/utils.ts initial modified`

```
```

## `vt run sub-pkg#dotdot-positive-negative`

```
~/packages/sub-pkg$ vtt print-file ../shared/src/utils.ts ../shared/dist/output.js ○ cache miss: 'packages/shared/src/utils.ts' modified, executing
export const shared = 'modified';
// initial output
```
