# positive_auto_negative___hit_on_excluded_file

Under combined positive + `auto` + negative config, a negative glob should
still filter files contributed by either source.

## `vt run positive-auto-negative`

```
$ vtt print-file src/main.ts
export const main = 'initial';
```

## `vtt replace-file-content src/main.test.ts main modified`

```
```

## `vt run positive-auto-negative`

```
$ vtt print-file src/main.ts ◉ cache hit, replaying
export const main = 'initial';

---
vt run: cache hit.
```
