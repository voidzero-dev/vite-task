# typescript_incremental

TypeScript 7's `tsc --incremental` keeps what it learned in `tsconfig.tsbuildinfo`. A run without the file probes for it and writes it, a run after an edit reads and rewrites it, and a run with nothing changed only reads it.

## `vt run typecheck`

no tsconfig.tsbuildinfo yet: tsc probes for it and writes it

```
$ tsc --incremental --noEmit
```

## `vt run typecheck`

nothing changed

```
$ tsc --incremental --noEmit ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt rm tsconfig.tsbuildinfo`

remove the build info, as on a clean checkout

```
```

## `vt run typecheck`

```
$ tsc --incremental --noEmit ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt stat-file tsconfig.tsbuildinfo`

```
tsconfig.tsbuildinfo: exists
```

## `vtt replace-file-content src/main.ts initial modified`

```
```

## `vt run typecheck`

```
$ tsc --incremental --noEmit ○ cache miss: 'src/main.ts' modified, executing
```
