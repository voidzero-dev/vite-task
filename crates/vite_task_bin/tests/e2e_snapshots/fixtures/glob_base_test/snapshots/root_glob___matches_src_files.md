# root_glob___matches_src_files

A root-package glob should match files under the package's own `src/` —
modifying such a file invalidates the cache.

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
