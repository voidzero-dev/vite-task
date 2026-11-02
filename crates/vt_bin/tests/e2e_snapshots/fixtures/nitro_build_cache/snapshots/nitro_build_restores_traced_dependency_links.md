# nitro_build_restores_traced_dependency_links

A real Nitro server build traces `dep-a` and `dep-b`, which depend on different versions of `tslib` (small stand-ins committed in the fixture's `node_modules`). nf3 writes each version under `.output/server/node_modules/.nf3/tslib@<version>`, links `node_modules/tslib` to the hoisted one, and links each package's `node_modules/tslib` to the one it needs. A cache hit restores those links as links, so each package still resolves its own version from the restored output. On Windows, nf3 creates junctions, whose targets are absolute paths.

## `vt run build`

first run — cache miss, Nitro builds .output/

```
$ CONSOLA_LEVEL=0 nitro build
```

## `node check.cjs`

nf3 linked each package's tslib into .nf3, so each resolves its own version

```
tslib: link to .nf3/tslib@2.8.1
dep-a/node_modules/tslib: link to ../../.nf3/tslib@1.14.1
dep-b/node_modules/tslib: link to ../../.nf3/tslib@2.8.1
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

the restored output has the same links, not copies, and resolves the same versions

```
tslib: link to .nf3/tslib@2.8.1
dep-a/node_modules/tslib: link to ../../.nf3/tslib@1.14.1
dep-b/node_modules/tslib: link to ../../.nf3/tslib@2.8.1
dep-a: tslib 1.14.1
dep-b: tslib 2.8.1
hoisted: tslib 2.8.1
```
