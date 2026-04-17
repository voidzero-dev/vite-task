# subpackage_auto_with_negative___miss_on_non_excluded_inferred_file

Test that negative input globs work correctly for subpackages.
Bug: negative globs were matched against workspace-relative paths
instead of package-relative paths, so exclusions like !dist/**
failed for subpackages.

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
