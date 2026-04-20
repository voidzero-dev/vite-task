# positive_negative_globs___hit_on_excluded_file

A file matched by the negative glob should be excluded from fingerprinting
even if it also matches a positive glob.

## `vt run positive-negative-globs`

```
$ vtt print-file src/main.ts
export const main = 'initial';
```

## `vtt replace-file-content src/main.test.ts main modified`

```
```

## `vt run positive-negative-globs`

```
$ vtt print-file src/main.ts ◉ cache hit, replaying
export const main = 'initial';

---
vt run: cache hit.
```
