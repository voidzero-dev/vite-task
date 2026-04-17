# auto_only___hit_on_non_inferred_file_change

Test all input configuration combinations for cache behavior

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
