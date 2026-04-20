# recursive_build_skips_root_self_reference

`vt run -r build` from the workspace root, when root's own `build` script is
itself `vt run -r build`, should hit the skip rule: the nested expansion is
the same query, so root's step is skipped and only packages a and b run.

## `vt run -r build`

only a and b run, root is skipped

```
~/packages/a$ echo building-a ⊘ cache disabled
building-a

~/packages/b$ echo building-b ⊘ cache disabled
building-b

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```
