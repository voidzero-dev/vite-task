# hide_pending_uploads

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server --stall /store vt run build`

The backend never responds to the upload. vt run waits for it until Ctrl-C, without the message about pending uploads.

**→ expect-milestone:** `stalled`

```
$ vtt write-file dist/output.txt built
```

**← write-key:** `ctrl-c`

```
$ vtt write-file dist/output.txt built

---
vt run: remote-cache#build not uploaded to the remote cache: interrupted. (Run `vt run --last-details` for full details)
[remote-cache] POST /fetch 404
```
