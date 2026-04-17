# empty_input___hit_despite_file_changes

Test all input configuration combinations for cache behavior

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
