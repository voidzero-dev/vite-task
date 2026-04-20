# subpackage_glob___unmatched_directory_in_subpackage

Changes to a sibling directory within the subpackage that the glob does not
cover (e.g. `other/`) should leave the cache hit intact.

## `vt run sub-pkg#sub-glob-test`

```
~/packages/sub-pkg$ vtt print-file src/sub.ts
export const sub = 'initial';
```

## `vtt replace-file-content packages/sub-pkg/other/other.ts initial modified`

```
```

## `vt run sub-pkg#sub-glob-test`

```
~/packages/sub-pkg$ vtt print-file src/sub.ts ◉ cache hit, replaying
export const sub = 'initial';

---
vt run: cache hit.
```
