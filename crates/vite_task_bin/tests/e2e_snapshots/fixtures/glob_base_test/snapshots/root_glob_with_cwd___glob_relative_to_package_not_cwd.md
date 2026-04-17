# root_glob_with_cwd___glob_relative_to_package_not_cwd

Test glob base directory behavior
Globs are relative to PACKAGE directory, NOT task cwd
No special cross-package filtering - just normal relative path matching

## `vt run root-glob-with-cwd`

```
~/src$ vtt print-file root.ts
export const root = 'initial';
```

## `vtt replace-file-content src/root.ts initial modified`

```
```

## `vt run root-glob-with-cwd`

```
~/src$ vtt print-file root.ts ○ cache miss: 'src/root.ts' modified, executing
export const root = 'modified';
```
