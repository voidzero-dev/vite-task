# fast_fail_during_fetch

## `vtt stalled-remote-cache vt run fail-during-build`

The endpoint never responds. fail exits while build's fetch is in flight, which stops the fetch, and build doesn't start.

**Exit code:** 1

```
$ vtt exit 1 ⊘ cache disabled
```
