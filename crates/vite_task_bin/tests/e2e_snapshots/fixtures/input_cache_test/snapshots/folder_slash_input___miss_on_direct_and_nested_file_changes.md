# folder_slash_input___miss_on_direct_and_nested_file_changes

A trailing-slash input (`src/`) should expand to `src/**`, so both direct
and nested file changes under it invalidate the cache.

## `vt run folder-slash-input`

```
$ vtt print-file src/main.ts
export const main = 'initial';
```

## `vtt replace-file-content src/main.ts initial modified`

```
```

## `vt run folder-slash-input`

```
$ vtt print-file src/main.ts ○ cache miss: 'src/main.ts' modified, executing
export const main = 'modified';
```

## `vtt replace-file-content src/main.ts modified initial`

```
```

## `vt run folder-slash-input`

```
$ vtt print-file src/main.ts ○ cache miss: 'src/main.ts' modified, executing
export const main = 'initial';
```

## `vtt replace-file-content src/sub/nested.ts initial modified`

```
```

## `vt run folder-slash-input`

```
$ vtt print-file src/main.ts ○ cache miss: 'src/sub/nested.ts' modified, executing
export const main = 'initial';
```
