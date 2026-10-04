# fast_fail_during_fetch

## `VP_REMOTE_CACHE_URL=http://127.0.0.1:0/projects/test vtt stalled-remote-cache --stall /fetch vt run fail-during-build`

The proxy never answers the fetch. fail exits while build's fetch is in flight, which stops the fetch, and build doesn't start.

**Exit code:** 1

```
$ vtt exit 1 ⊘ cache disabled
```
