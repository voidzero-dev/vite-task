# fast_fail_kills_tasks_in_nested_continue_run

A nested `vt run --continue` doesn't protect its tasks from an outer run that fast-fails. `fails-beside-nested` (in the outer run) and `hangs-after-barrier` (in the nested run) share a 2-way barrier, so the hanging task is running when the outer task fails. Completing without a timeout shows the outer failure killed it.

## `vt run failure-stops-nested-continue`

fails-beside-nested fails, so hangs-after-barrier in the nested run is killed

**Exit code:** 1

```
~/packages/pkg-c$ vtt barrier ../../.barrier outer 2 --exit=4
~/packages/pkg-c$ vtt barrier ../../.barrier outer 2 --hang


---
vt run: 0/2 cache hit (0%), 0/2 successful. (Run `vt run --last-details` for full details)
```
