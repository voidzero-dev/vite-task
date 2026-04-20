# independent_tasks_run_concurrently

Two packages with no dependency relationship should run concurrently. Both tasks
participate in a 2-way barrier, so sequential execution would hang forever and
time out.

## `vt run -r build`

```
~/packages/a$ vtt barrier ../../.barrier sync 2 ⊘ cache disabled
~/packages/b$ vtt barrier ../../.barrier sync 2 ⊘ cache disabled


---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```
