# continue_is_inherited_by_vt_run_spawned_by_task

`--continue` also reaches a `vt run` that a task starts as a separate process instead of one expanded in place, here through `vtt pipe-stdin`.

## `vt run --continue spawned-plain`

the spawned vt run inherits --continue: fail-3 and fail-5 both run

**Exit code:** 1

```
~/packages/pkg-c$ vtt pipe-stdin -- vt run --concurrency-limit 1 two-failures ⊘ cache disabled
~/packages/pkg-c$ vtt exit 3

~/packages/pkg-c$ vtt exit 5

---
vt run: 0/2 cache hit (0%), 0/2 successful. (Run `vt run --last-details` for full details)
```
