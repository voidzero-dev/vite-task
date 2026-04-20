# auto_only___miss_on_inferred_file_change

With `auto: true`, files read by the command (via fspy) should be fingerprinted
and a change to such a file should invalidate the cache.

## `vt run auto-only`

```
$ vtt print-file src/main.ts
export const main = 'initial';
```

## `vtt replace-file-content src/main.ts initial modified`

```
```

## `vt run auto-only`

```
$ vtt print-file src/main.ts ○ cache miss: 'src/main.ts' modified, executing
export const main = 'modified';
```
