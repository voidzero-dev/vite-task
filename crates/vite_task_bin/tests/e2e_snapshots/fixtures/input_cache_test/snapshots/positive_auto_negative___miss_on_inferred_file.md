# positive_auto_negative___miss_on_inferred_file

Under combined positive + `auto` + negative config, modifying a file that
only the `auto` inference discovered should still invalidate the cache.

## `vt run positive-auto-negative`

```
$ vtt print-file src/main.ts
export const main = 'initial';
```

## `vtt replace-file-content src/main.ts initial modified`

```
```

## `vt run positive-auto-negative`

```
$ vtt print-file src/main.ts ○ cache miss: 'src/main.ts' modified, executing
export const main = 'modified';
```
