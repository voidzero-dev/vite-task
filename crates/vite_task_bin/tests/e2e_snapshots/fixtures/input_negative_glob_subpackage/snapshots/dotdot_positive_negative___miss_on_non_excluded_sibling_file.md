# dotdot_positive_negative___miss_on_non_excluded_sibling_file

Test that negative input globs work correctly for subpackages.
Bug: negative globs were matched against workspace-relative paths
instead of package-relative paths, so exclusions like !dist/**
failed for subpackages.

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
