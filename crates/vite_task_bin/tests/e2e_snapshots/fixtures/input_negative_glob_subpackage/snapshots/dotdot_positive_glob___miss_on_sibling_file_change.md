# dotdot_positive_glob___miss_on_sibling_file_change

Test that negative input globs work correctly for subpackages.
Bug: negative globs were matched against workspace-relative paths
instead of package-relative paths, so exclusions like !dist/**
failed for subpackages.

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
