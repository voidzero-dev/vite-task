# continue_exits_1_when_several_tasks_fail

With `--continue`, independent failing tasks all run. With more than one failure, the run exits 1 instead of a task's exit code.

## `vt run --continue two-failures`

fail-3 and fail-5 both run

**Exit code:** 1

```
~/packages/pkg-c$ vtt exit 3
~/packages/pkg-c$ vtt exit 5


---
vt run: 0/2 cache hit (0%), 0/2 successful. (Run `vt run --last-details` for full details)
```
