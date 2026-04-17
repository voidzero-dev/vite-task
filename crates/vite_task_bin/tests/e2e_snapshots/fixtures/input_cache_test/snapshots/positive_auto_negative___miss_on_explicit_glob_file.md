# positive_auto_negative___miss_on_explicit_glob_file

Test all input configuration combinations for cache behavior

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
