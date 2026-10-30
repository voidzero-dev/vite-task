# nitro_build_restores_traced_dependency_links

A real Nitro server build traces `dep-a` and `dep-b`, which depend on different versions of `tslib` (small stand-ins committed in the fixture's `node_modules`). nf3 writes each version under `.output/server/node_modules/.nf3/tslib@<version>`, links `node_modules/tslib` to the hoisted one, and links each package's `node_modules/tslib` to the one it needs. A cache hit restores those links, so each package still resolves its own version from the restored output. On Windows, nf3 creates junctions, whose targets are absolute paths.

## `vt run build`

first run — cache miss, Nitro builds .output/

```
$ CONSOLA_LEVEL=0 nitro build
```

## `node check.cjs`

each package resolves its own tslib through nf3's links

```
dep-a: tslib 1.14.1
dep-b: tslib 2.8.1
hoisted: tslib 2.8.1
```

## `vtt rm -rf .output`

delete .output/ to prove the restore is real

```
```

## `vt run build`

second run — cache hit, restores .output/ with its links

```
$ CONSOLA_LEVEL=0 nitro build ◉ cache hit, replaying

---
vt run: cache hit.
```

## `node check.cjs`

the restored output resolves the same versions

```
dep-a: tslib 1.14.1
dep-b: tslib 2.8.1
hoisted: tslib 2.8.1
```
