# failure_kills_concurrent_tasks

Tests that independent tasks execute concurrently.
Packages a and b have no dependency relationship.
Both use a barrier that requires 2 participants — if run sequentially,
the first would wait forever and the test would timeout.

## `vt run -r test`

**Exit code:** 1

```
~/packages/a$ vtt barrier ../../.barrier test-sync 2 --exit=1 ⊘ cache disabled
~/packages/b$ vtt barrier ../../.barrier test-sync 2 --hang ⊘ cache disabled


---
vt run: 0/2 cache hit (0%), 2 failed. (Run `vt run --last-details` for full details)
```
