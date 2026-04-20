# dotdot_positive_glob___hit_on_unmatched_file_change

A `../` positive glob should only match the paths it names — sibling files
outside the glob (e.g. sibling `dist/`) should not affect the cache.

## `vt run sub-pkg#dotdot-positive`

```
~/packages/sub-pkg$ vtt print-file ../shared/src/utils.ts
export const shared = 'initial';
```

## `vtt replace-file-content packages/shared/dist/output.js initial modified`

```
```

## `vt run sub-pkg#dotdot-positive`

```
~/packages/sub-pkg$ vtt print-file ../shared/src/utils.ts ◉ cache hit, replaying
export const shared = 'initial';

---
vt run: cache hit.
```
