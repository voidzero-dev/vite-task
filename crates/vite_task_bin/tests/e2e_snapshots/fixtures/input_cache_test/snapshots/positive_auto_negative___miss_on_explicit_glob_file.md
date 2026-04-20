# positive_auto_negative___miss_on_explicit_glob_file

When positive, `auto`, and negative globs are combined, modifying a file
matched by the explicit positive glob should invalidate the cache.

## `vt run positive-auto-negative`

```
$ vtt print-file src/main.ts
export const main = 'initial';
```

## `vtt replace-file-content package.json inputs-cache-test modified-pkg`

```
```

## `vt run positive-auto-negative`

```
$ vtt print-file src/main.ts ○ cache miss: 'package.json' modified, executing
export const main = 'initial';
```
