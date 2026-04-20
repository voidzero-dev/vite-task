# positive_negative_globs___miss_on_non_excluded_file

A file matched by a positive glob and not by any negative glob should still
invalidate the cache when modified.

## `vt run positive-negative-globs`

```
$ vtt print-file src/main.ts
export const main = 'initial';
```

## `vtt replace-file-content src/main.ts initial modified`

```
```

## `vt run positive-negative-globs`

```
$ vtt print-file src/main.ts ○ cache miss: 'src/main.ts' modified, executing
export const main = 'modified';
```
