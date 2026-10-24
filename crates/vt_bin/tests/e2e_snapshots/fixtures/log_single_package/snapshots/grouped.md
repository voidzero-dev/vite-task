# grouped

In a single-package workspace, `--log=grouped` labels each block with `[task]` instead of `[pkg#task]`.

## `vt run --log=grouped build`

```
[lint] $ echo lint ⊘ cache disabled
── [lint] ──
lint

[build] $ echo build ⊘ cache disabled
── [build] ──
build

---
vt run: 2/2 successful. (Run `vt run --last-details` for full details)
```
