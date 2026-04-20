# dotdot_positive_glob___miss_on_sibling_file_change

A `../` positive glob should reach into sibling packages; modifying a matched
sibling file should invalidate the cache.

## `vt run sub-pkg#dotdot-positive`

```
~/packages/sub-pkg$ vtt print-file ../shared/src/utils.ts
export const shared = 'initial';
```

## `vtt replace-file-content packages/shared/src/utils.ts initial modified`

```
```

## `vt run sub-pkg#dotdot-positive`

```
~/packages/sub-pkg$ vtt print-file ../shared/src/utils.ts ○ cache miss: 'packages/shared/src/utils.ts' modified, executing
export const shared = 'modified';
```
