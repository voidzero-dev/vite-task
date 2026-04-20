# folder_slash_input___hit_on_file_outside_directory

With a trailing-slash input (`src/`), files outside the directory should not
be part of the fingerprint.

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
