# cache_hit_then_miss_on_file_change

Test that glob meta characters in package paths are correctly escaped by wax::escape.
Without escaping, "packages/[lib]/src/**/*.ts" would interpret [lib] as a character
class matching 'l', 'i', or 'b' instead of the literal directory name.

## `vt run [lib]#build`

```
~/packages/[lib]$ vtt print-file src/main.ts
export const lib = 'initial';
```

## `vt run [lib]#build`

```
~/packages/[lib]$ vtt print-file src/main.ts ◉ cache hit, replaying
export const lib = 'initial';

---
vt run: cache hit.
```

## `vtt replace-file-content packages/[lib]/src/main.ts initial modified`

```
```

## `vt run [lib]#build`

```
~/packages/[lib]$ vtt print-file src/main.ts ○ cache miss: 'packages/[lib]/src/main.ts' modified, executing
export const lib = 'modified';
```
