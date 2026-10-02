# fast_fail_during_upload

## `VP_REMOTE_CACHE=read-write vtt stalled-remote-cache --fetch-miss vt run fail-after-build`

The endpoint never responds to the upload. fail-after-build exits after build finishes, which doesn't cancel build's upload, so vt run waits for it until Ctrl-C.

**Exit code:** 1

**→ expect-milestone:** `uploads-pending`

```
$ vtt write-file dist/output.txt built

$ vtt exit 1 ⊘ cache disabled

Waiting for 1 remote cache upload to finish (Ctrl-C to cancel)...
```

**← write-key:** `ctrl-c`

```
$ vtt write-file dist/output.txt built

$ vtt exit 1 ⊘ cache disabled

Waiting for 1 remote cache upload to finish (Ctrl-C to cancel)...
---
vt run: 0/2 cache hit (0%), 1 failed. remote-cache#build not uploaded to the remote cache: interrupted. (Run `vt run --last-details` for full details)
```
