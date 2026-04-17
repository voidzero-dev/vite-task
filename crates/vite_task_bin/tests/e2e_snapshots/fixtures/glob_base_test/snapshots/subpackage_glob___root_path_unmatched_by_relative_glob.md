# subpackage_glob___root_path_unmatched_by_relative_glob

Test glob base directory behavior
Globs are relative to PACKAGE directory, NOT task cwd
No special cross-package filtering - just normal relative path matching

## `vt run sub-pkg#sub-glob-test`

```
~/packages/sub-pkg$ vtt print-file src/sub.ts
export const sub = 'initial';
```

## `vtt replace-file-content src/root.ts initial modified`

```
```

## `vt run sub-pkg#sub-glob-test`

```
~/packages/sub-pkg$ vtt print-file src/sub.ts ◉ cache hit, replaying
export const sub = 'initial';

---
vt run: cache hit.
```
