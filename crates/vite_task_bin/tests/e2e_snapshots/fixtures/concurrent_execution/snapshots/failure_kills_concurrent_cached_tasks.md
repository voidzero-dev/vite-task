# failure_kills_concurrent_cached_tasks

Same failure-cancellation scenario, but with `--cache` so execution goes through
the piped stdio / fspy path (spawn_with_tracking) instead of the inherited-stdio
path.

## `vt run -r --cache test`

**Exit code:** 1

```
~/packages/a$ vtt barrier ../../.barrier test-sync 2 --exit=1
~/packages/b$ vtt barrier ../../.barrier test-sync 2 --hang


---
vt run: 0/2 cache hit (0%), 2 failed. (Run `vt run --last-details` for full details)
```
