# independent_tasks_run_concurrently

Tests that independent tasks execute concurrently.
Packages a and b have no dependency relationship.
Both use a barrier that requires 2 participants — if run sequentially,
the first would wait forever and the test would timeout.

## `vt run -r build`

```
~/packages/a$ vtt barrier ../../.barrier sync 2 ⊘ cache disabled
~/packages/b$ vtt barrier ../../.barrier sync 2 ⊘ cache disabled


---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```
