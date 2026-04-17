# recursive_build_skips_root_self_reference

Tests that workspace root self-referencing tasks don't cause infinite recursion.
Root build = `vt run -r build` (delegates to all packages recursively).

Skip rule: `vt run -r build` from root produces the same query as the
nested `vt run -r build` in root's script, so root's expansion is skipped.
Only packages a and b actually run.

Prune rule: `vt run build` from root produces a ContainingPackage query,
but root's script `vt run -r build` produces an All query. The queries
differ so the skip rule doesn't fire. Instead the prune rule removes root
from the nested result, leaving only a and b.

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
