# transitive_build_from_lib_runs_only_its_dependencies

`vt run -t build` from an intermediate package should limit the run to that
package and its own dependencies — `@topo/app` must not run.

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
