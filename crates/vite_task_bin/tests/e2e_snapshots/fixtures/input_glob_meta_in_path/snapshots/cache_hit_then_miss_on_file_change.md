# cache_hit_then_miss_on_file_change

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
