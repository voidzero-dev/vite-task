# root_glob___unmatched_directory

Modifying a file outside the glob pattern (e.g. `other/`) should leave the
cache hit intact.

## `vt run root-glob-test`

```
$ vtt print-file src/root.ts
export const root = 'initial';
```

## `vtt replace-file-content other/other.ts initial modified`

```
```

## `vt run root-glob-test`

```
$ vtt print-file src/root.ts ◉ cache hit, replaying
export const root = 'initial';

---
vt run: cache hit.
```
