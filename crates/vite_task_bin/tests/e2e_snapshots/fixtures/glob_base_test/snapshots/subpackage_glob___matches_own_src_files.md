# subpackage_glob___matches_own_src_files

A subpackage's glob should match files under its own `src/` directory.

## `vt run sub-pkg#sub-glob-test`

```
~/packages/sub-pkg$ vtt print-file src/sub.ts
export const sub = 'initial';
```

## `vtt replace-file-content packages/sub-pkg/src/sub.ts initial modified`

```
```

## `vt run sub-pkg#sub-glob-test`

```
~/packages/sub-pkg$ vtt print-file src/sub.ts ○ cache miss: 'packages/sub-pkg/src/sub.ts' modified, executing
export const sub = 'modified';
```
