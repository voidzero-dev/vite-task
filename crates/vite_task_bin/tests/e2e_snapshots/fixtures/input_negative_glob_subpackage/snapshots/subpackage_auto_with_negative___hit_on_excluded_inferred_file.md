# subpackage_auto_with_negative___hit_on_excluded_inferred_file

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

## `vtt replace-file-content packages/sub-pkg/dist/output.js initial modified`

```
```

## `vt run sub-pkg#auto-with-negative`

```
~/packages/sub-pkg$ vtt print-file src/main.ts dist/output.js ◉ cache hit, replaying
export const main = 'initial';
// initial output

---
vt run: cache hit.
```
