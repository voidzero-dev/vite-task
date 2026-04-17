# positive_auto_negative___miss_on_inferred_file

Test all input configuration combinations for cache behavior

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
