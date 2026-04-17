# root_glob___matches_src_files

Test glob base directory behavior
Globs are relative to PACKAGE directory, NOT task cwd
No special cross-package filtering - just normal relative path matching

## `vt run root-glob-test`

```
$ vtt print-file src/root.ts
export const root = 'initial';
```

## `vtt replace-file-content src/root.ts initial modified`

```
```

## `vt run root-glob-test`

```
$ vtt print-file src/root.ts ○ cache miss: 'src/root.ts' modified, executing
export const root = 'modified';
```
