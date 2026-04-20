# empty_input___hit_despite_file_changes

With `input: []`, no file changes should ever invalidate the cache — only
command/env changes can.

## `vt run empty-inputs`

```
$ vtt print-file ./src/main.ts
export const main = 'initial';
```

## `vtt replace-file-content src/main.ts initial modified`

```
```

## `vt run empty-inputs`

```
$ vtt print-file ./src/main.ts ◉ cache hit, replaying
export const main = 'initial';

---
vt run: cache hit.
```
