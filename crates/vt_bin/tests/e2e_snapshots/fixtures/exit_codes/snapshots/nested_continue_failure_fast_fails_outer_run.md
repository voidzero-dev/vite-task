# nested_continue_failure_fast_fails_outer_run

When a nested `vt run --continue` finishes with a failure, its task fails, and an outer run without `--continue` fast-fails as usual. `fails-after-barrier` (in the nested run) and `hangs-beside-nested` (in the outer run) share a 2-way barrier, so the hanging task is running when the nested run fails. Completing without a timeout shows the outer run killed it.

## `vt run nested-failure-stops-outer`

the nested run fails, so the outer run kills hangs-beside-nested

**Exit code:** 1

```
~/packages/pkg-c$ vtt barrier ../../.barrier nested 2 --exit=3
~/packages/pkg-c$ vtt barrier ../../.barrier nested 2 --hang


---
vt run: 0/2 cache hit (0%), 0/2 successful. (Run `vt run --last-details` for full details)
```
