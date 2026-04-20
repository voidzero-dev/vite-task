# transitive_build_from_app_runs_all_dependencies

`vt run -t build` from the leaf package should include the full transitive
closure of its dependencies in topological order.

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
