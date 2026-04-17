# root_glob___subpackage_path_unmatched_by_relative_glob

Test glob base directory behavior
Globs are relative to PACKAGE directory, NOT task cwd
No special cross-package filtering - just normal relative path matching

## `vt run root-glob-test`

```
$ vtt print-file src/root.ts
export const root = 'initial';
```

## `vtt replace-file-content packages/sub-pkg/src/sub.ts initial modified`

```
```

## `vt run root-glob-test`

```
$ vtt print-file src/root.ts ◉ cache hit, replaying
export const root = 'initial';

---
vt run: cache hit.
```
