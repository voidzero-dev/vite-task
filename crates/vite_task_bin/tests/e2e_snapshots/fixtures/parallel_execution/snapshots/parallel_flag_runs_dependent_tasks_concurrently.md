# parallel_flag_runs_dependent_tasks_concurrently

Package b depends on a, so without --parallel they run sequentially.
Both use a barrier requiring 2 participants — if run sequentially the
first would wait forever and the test would timeout.
--parallel discards dependency edges, allowing both to run at once.

## `vt run -r --parallel build`

```
~/packages/a$ vtt barrier ../../.barrier sync 2 ⊘ cache disabled
~/packages/b$ vtt barrier ../../.barrier sync 2 ⊘ cache disabled


---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```
