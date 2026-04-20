# dotdot_positive_negative___hit_on_excluded_sibling_file

A `../`-prefixed negative glob should correctly exclude files in the sibling
package's `dist/` directory.

## `vt run sub-pkg#dotdot-positive-negative`

```
~/packages/sub-pkg$ vtt print-file ../shared/src/utils.ts ../shared/dist/output.js
export const shared = 'initial';
// initial output
```

## `vtt replace-file-content packages/shared/dist/output.js initial modified`

```
```

## `vt run sub-pkg#dotdot-positive-negative`

```
~/packages/sub-pkg$ vtt print-file ../shared/src/utils.ts ../shared/dist/output.js ◉ cache hit, replaying
export const shared = 'initial';
// initial output

---
vt run: cache hit.
```
