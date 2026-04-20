# dotdot_auto_negative___hit_on_excluded_sibling_inferred_file

A `!../shared/dist/**` negative glob should filter inferred reads that land
in the sibling package's `dist/` directory.

## `vt run sub-pkg#dotdot-auto-negative`

```
~/packages/sub-pkg$ vtt print-file ../shared/src/utils.ts ../shared/dist/output.js
export const shared = 'initial';
// initial output
```

## `vtt replace-file-content packages/shared/dist/output.js initial modified`

```
```

## `vt run sub-pkg#dotdot-auto-negative`

```
~/packages/sub-pkg$ vtt print-file ../shared/src/utils.ts ../shared/dist/output.js ◉ cache hit, replaying
export const shared = 'initial';
// initial output

---
vt run: cache hit.
```
