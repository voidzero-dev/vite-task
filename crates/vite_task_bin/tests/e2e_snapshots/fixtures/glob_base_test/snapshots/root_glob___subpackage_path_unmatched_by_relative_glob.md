# root_glob___subpackage_path_unmatched_by_relative_glob

A relative glob anchored at the root package should not reach into
`packages/<sub>/src/` — a subpackage file change stays a cache hit.

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
