# subpackage_glob___root_path_unmatched_by_relative_glob

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
