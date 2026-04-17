# transitive_build_from_lib_runs_only_its_dependencies

Tests that tasks execute in dependency (topological) order.
Dependency chain: @topo/core <- @topo/lib <- @topo/app

## `vt run -t build`

core -> lib

```
~/packages/core$ echo 'Building core' ⊘ cache disabled
Building core

~/packages/lib$ echo 'Building lib' ⊘ cache disabled
Building lib

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```
