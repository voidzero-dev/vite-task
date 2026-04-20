# auto_only___hit_on_non_inferred_file_change

With `auto: true`, a file never read by the command should not affect the
cache, even if it sits next to files that are read.

## `vt run auto-only`

```
$ vtt print-file src/main.ts
export const main = 'initial';
```

## `vtt replace-file-content src/utils.ts initial modified`

```
```

## `vt run auto-only`

```
$ vtt print-file src/main.ts ◉ cache hit, replaying
export const main = 'initial';

---
vt run: cache hit.
```
