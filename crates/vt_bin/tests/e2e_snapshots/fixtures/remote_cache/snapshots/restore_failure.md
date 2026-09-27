# restore_failure

## `VP_REMOTE_CACHE=read-write remote-cache-server vt run build`

```
$ vtt write-file dist/output.txt built

[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `vt cache clean`

```
```

## `vtt rm -rf dist`

```
```

## `vtt write-file dist file`

A file where the output directory goes makes restoring fail.

```
```

## `remote-cache-server vt run build`

The remote hit can't be restored, so the task fails.

**Exit code:** 1

```
$ vtt write-file dist/output.txt built ◉ remote cache hit, replaying
✗ Cache restore failed: failed to extract the output archive: failed to unpack `<workspace>/dist/output.txt`: failed to unpack `dist/output.txt` into `<workspace>/dist/output.txt`: <os error>

[remote-cache] POST /fetch 200 exact
[remote-cache] GET /blob/1 200
```

## `vt run --last-details`

The details include the underlying error.

**Exit code:** 1

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 0 cache hits • 0 cache misses • 1 failed
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] remote-cache#build: $ vtt write-file dist/output.txt built
      → Remote cache hit, but the outputs couldn't be restored
      ✗ Error: Cache restore failed
        ↳ failed to extract the output archive
        ↳ failed to unpack `<workspace>/dist/output.txt`
        ↳ failed to unpack `dist/output.txt` into `<workspace>/dist/output.txt`
        ↳ <os error>
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vtt list-dir node_modules/.vite/task-cache --ext .tar.zst --recursive`

The downloaded archive was removed along with the entry.

```
```

## `vtt rm dist`

```
```

## `remote-cache-server vt run build`

The local entry was removed, so the remote entry is fetched and restored again.

```
$ vtt write-file dist/output.txt built ◉ remote cache hit, replaying

---
vt run: remote cache hit.
[remote-cache] POST /fetch 200 exact
[remote-cache] GET /blob/1 200
```
