# transitive_build_from_app_runs_all_dependencies

Tests that tasks execute in dependency (topological) order.
Dependency chain: @topo/core <- @topo/lib <- @topo/app

## `vt run -t build`

core -> lib -> app

```
~/packages/core$ echo 'Building core' ⊘ cache disabled
Building core

~/packages/lib$ echo 'Building lib' ⊘ cache disabled
Building lib

~/packages/app$ echo 'Building app' ⊘ cache disabled
Building app

---
vt run: 0/3 cache hit (0%). (Run `vt run --last-details` for full details)
```
