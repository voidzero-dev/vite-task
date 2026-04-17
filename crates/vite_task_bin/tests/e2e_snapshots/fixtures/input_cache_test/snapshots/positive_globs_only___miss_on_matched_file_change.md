# positive_globs_only___miss_on_matched_file_change

Test all input configuration combinations for cache behavior

## `vt run positive-globs-only`

```
$ vtt print-file src/main.ts
export const main = 'initial';
```

## `vtt replace-file-content src/main.ts initial modified`

```
```

## `vt run positive-globs-only`

```
$ vtt print-file src/main.ts ○ cache miss: 'src/main.ts' modified, executing
export const main = 'modified';
```
