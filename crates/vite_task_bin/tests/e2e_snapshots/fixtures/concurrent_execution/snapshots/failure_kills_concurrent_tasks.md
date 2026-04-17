# failure_kills_concurrent_tasks

## `vt run -r test`

**Exit code:** 1

```
~/packages/a$ vtt barrier ../../.barrier test-sync 2 --exit=1 ⊘ cache disabled
~/packages/b$ vtt barrier ../../.barrier test-sync 2 --hang ⊘ cache disabled


---
vt run: 0/2 cache hit (0%), 2 failed. (Run `vt run --last-details` for full details)
```
