# recursive_build_runs_dependencies_before_dependents

`vt run -r build` across the workspace should execute packages in topological order: `@topo/core` before `@topo/lib` before `@topo/app`.

## `vt run -r build`

core -> lib -> app

```
~/packages/core$ echo 'Building core' ⊘ cache disabled
Building core

~/packages/lib$ echo 'Building lib' ⊘ cache disabled
Building lib

~/packages/app$ echo 'Building app' ⊘ cache disabled
Building app

---
vt run: (Run `vt run --last-details` for full details)
```
