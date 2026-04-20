# folder_input___hit_despite_file_changes_and_folder_deletion

A bare directory name as input (`src`) matches only the directory entry,
not its contents — neither file edits nor folder deletion affect the cache.

## `vt run folder-input`

```
$ vtt print-file src/main.ts
export const main = 'initial';
```

## `vtt replace-file-content src/main.ts initial modified`

```
```

## `vt run folder-input`

```
$ vtt print-file src/main.ts ◉ cache hit, replaying
export const main = 'initial';

---
vt run: cache hit.
```

## `vtt rm -rf src`

```
```

## `vt run folder-input`

```
$ vtt print-file src/main.ts ◉ cache hit, replaying
export const main = 'initial';

---
vt run: cache hit.
```
