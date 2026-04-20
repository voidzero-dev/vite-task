# subpackage_auto_with_negative___hit_on_excluded_inferred_file

A subpackage's `!dist/**` exclusion should apply package-relative, not
workspace-relative — modifying the subpackage's own `dist/` file stays a
cache hit. (Regression: the exclusion used to resolve against the workspace
root.)

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
