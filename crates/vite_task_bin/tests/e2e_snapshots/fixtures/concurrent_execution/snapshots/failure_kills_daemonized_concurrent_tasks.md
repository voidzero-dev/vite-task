# failure_kills_daemonized_concurrent_tasks

## `vt run -r --cache daemon`

**Exit code:** 1

```
~/packages/a$ vtt barrier ../../.barrier daemon-sync 2 --exit=1
~/packages/b$ vtt barrier ../../.barrier daemon-sync 2 --daemonize


---
vt run: 0/2 cache hit (0%), 2 failed. (Run `vt run --last-details` for full details)
```
