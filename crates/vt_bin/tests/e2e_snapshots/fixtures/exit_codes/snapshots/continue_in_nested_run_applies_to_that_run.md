# continue_in_nested_run_applies_to_that_run

A `vt run --continue` in a task's script applies to the tasks it runs, even when the outer `vt run` has no `--continue`. With a concurrency limit of 1, fail-3 and fail-5 run one after the other, so the second only runs if the first failure doesn't cancel the run.

## `vt run nested-continue`

the nested run has --continue: fail-3 and fail-5 both run

**Exit code:** 1

```
~/packages/pkg-c$ vtt exit 3

~/packages/pkg-c$ vtt exit 5

---
vt run: 0/2 cache hit (0%), 0/2 successful. (Run `vt run --last-details` for full details)
```
