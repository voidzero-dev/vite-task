# subpackage_glob_with_cwd___glob_relative_to_package_not_cwd

Test glob base directory behavior
Globs are relative to PACKAGE directory, NOT task cwd
No special cross-package filtering - just normal relative path matching

## `vt run sub-pkg#sub-glob-with-cwd`

```
~/packages/sub-pkg/src$ vtt print-file sub.ts
export const sub = 'initial';
```

## `vtt replace-file-content packages/sub-pkg/src/sub.ts initial modified`

```
```

## `vt run sub-pkg#sub-glob-with-cwd`

```
~/packages/sub-pkg/src$ vtt print-file sub.ts ○ cache miss: 'packages/sub-pkg/src/sub.ts' modified, executing
export const sub = 'modified';
```
