# nitro_auto_outputs_restore_links

The same Nitro build with automatic input and output tracking only, no `input` or `output` globs. nf3 checks each path before linking it, so the links are among the paths the task read.

## `vt run build-auto`

first run — cache miss, Nitro builds .output/

```
$ CONSOLA_LEVEL=0 nitro build
```

## `node check.cjs`

```
tslib: link to .nf3/tslib@2.8.1
dep-a/node_modules/tslib: link to ../../.nf3/tslib@1.14.1
dep-b/node_modules/tslib: link to ../../.nf3/tslib@2.8.1
dep-a: tslib 1.14.1
dep-b: tslib 2.8.1
hoisted: tslib 2.8.1
```

## `vtt rm -rf .output`

remove .output/, as on a clean checkout

```
```

## `vt run build-auto`

several inputs are missing and vt names whichever it finds first, so the miss reason is redacted

```
$ CONSOLA_LEVEL=0 nitro build ◉ cache hit, replaying

---
vt run: cache hit.
```

## `node check.cjs`

the output has the same links, not copies, and resolves the same versions

```
tslib: link to .nf3/tslib@2.8.1
dep-a/node_modules/tslib: link to ../../.nf3/tslib@1.14.1
dep-b/node_modules/tslib: link to ../../.nf3/tslib@2.8.1
dep-a: tslib 1.14.1
dep-b: tslib 2.8.1
hoisted: tslib 2.8.1
```
