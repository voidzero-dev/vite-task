# build_from_root_prunes_root_from_nested_expansion

`vt run build` from root produces a `ContainingPackage` query, while root's
script (`vt run -r build`) produces an `All` query, so the skip rule does
not apply. The prune rule should instead remove root from the nested result,
again leaving only a and b.

## `vt run build`

only a and b run under root, root is pruned

```
~/packages/a$ echo building-a ⊘ cache disabled
building-a

~/packages/b$ echo building-b ⊘ cache disabled
building-b

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```
