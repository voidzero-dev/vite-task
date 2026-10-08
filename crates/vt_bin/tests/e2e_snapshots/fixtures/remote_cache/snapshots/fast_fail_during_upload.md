# fast_fail_during_upload

## `VP_REMOTE_CACHE=read-write remote-cache-server vtt stalled-remote-cache --stall /store vt run fail-after-build`

The proxy never forwards the upload. fail-after-build exits after build finishes, which doesn't cancel build's upload, so vt run waits for it until Ctrl-C.

**Exit code:** 1

**→ expect-milestone:** `uploads-pending`

```
$ vtt write-file dist/output.txt built

$ vtt exit 1 ⊘ cache disabled

Waiting for 1 remote cache upload to finish...
```

**← write-key:** `ctrl-c`

```
$ vtt write-file dist/output.txt built

$ vtt exit 1 ⊘ cache disabled

Waiting for 1 remote cache upload to finish...
---
vt run: 0/1 cache hit (0%), 1 failed. remote-cache#build not uploaded to the remote cache: interrupted. (Run `vt run --last-details` for full details)
[remote-cache] POST /fetch 404
```
