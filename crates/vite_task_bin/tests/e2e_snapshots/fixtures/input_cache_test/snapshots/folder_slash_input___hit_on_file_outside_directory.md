# folder_slash_input___hit_on_file_outside_directory

Test all input configuration combinations for cache behavior

## `vt run folder-slash-input`

```
$ vtt print-file src/main.ts
export const main = 'initial';
```

## `vtt replace-file-content test/main.test.ts outside modified`

```
```

## `vt run folder-slash-input`

```
$ vtt print-file src/main.ts ◉ cache hit, replaying
export const main = 'initial';

---
vt run: cache hit.
```
