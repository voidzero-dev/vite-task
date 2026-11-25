# upload_concurrency_cap

At most 4 concurrent remote cache uploads are in flight at once, and full upload slots do not block fetches.

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 VP_RUN_INTERNAL_REMOTE_CACHE_NO_BACKOFF=1 vtt mock-remote-cache --hold-stores 4 --release-after-fetches 7 --summary vt run t7`

The mock server holds stores on a Condvar until 4 are concurrently in flight and all 7 fetches have arrived, proving reads are unblocked and max_concurrent_stores stays capped at 4.

```
$ vtt print t1
t1

$ vtt print t2
t2

$ vtt print t3
t3

$ vtt print t4
t4

$ vtt print t5
t5

$ vtt print t6
t6

$ vtt print t7
t7

---
vt run: 0/7 cache hit (0%), 7/7 successful. (Run `vt run --last-details` for full details)
[remote-cache] POST /fetch 404 x7
[remote-cache] POST /store 200 x7
[remote-cache] max_concurrent_stores=4
```
