# failure_kills_concurrent_tasks

When one concurrent task fails, the sibling running under inherited stdio must
be cancelled. Task a exits 1 after the shared barrier, task b hangs after it —
completing without timeout proves cancellation killed b.

## `vt run -r test`

**Exit code:** 1

```
~/packages/a$ vtt barrier ../../.barrier test-sync 2 --exit=1 ⊘ cache disabled
~/packages/b$ vtt barrier ../../.barrier test-sync 2 --hang ⊘ cache disabled


---
vt run: 0/2 cache hit (0%), 2 failed. (Run `vt run --last-details` for full details)
```
