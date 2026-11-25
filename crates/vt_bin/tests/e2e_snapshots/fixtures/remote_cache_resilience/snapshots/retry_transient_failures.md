# retry_transient_failures

Transient 503, 429, connection-drop, and truncated-download errors are retried across fetch, store, and blob restore.

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 VP_RUN_INTERNAL_REMOTE_CACHE_NO_BACKOFF=1 vtt mock-remote-cache --state-dir .mock-cache --fault /fetch=503 --fault /fetch=drop --fault /store=503 --fault /store=429 --fault /store=drop vt run build`

Fetch recovers from 503 and connection drop to receive 404. Store retries 503, 429, and connection drop, re-streaming the output archive until 200.

```
$ vtt write-file dist/output.txt built

[remote-cache] POST /fetch 503
[remote-cache] POST /fetch drop
[remote-cache] POST /fetch 404
[remote-cache] POST /store 503
[remote-cache] POST /store 429
[remote-cache] POST /store drop
[remote-cache] POST /store 200
```

## `vt cache clean`

```
```

## `vtt rm -rf dist`

```
```

## `VP_RUN_INTERNAL_REMOTE_CACHE_NO_BACKOFF=1 vtt mock-remote-cache --state-dir .mock-cache --fault /fetch=503 --fault /blob=503 --fault /blob=truncate vt run build`

Fetch retries 503 and hits the stored entry. Blob download retries 503 and a mid-stream truncation, then restores the output.

```
$ vtt write-file dist/output.txt built ◉ remote cache hit, replaying

---
vt run: remote cache hit.
[remote-cache] POST /fetch 503
[remote-cache] POST /fetch 200
[remote-cache] GET /blob/1 503
[remote-cache] GET /blob/1 truncate
[remote-cache] GET /blob/1 200
```

## `vtt print-file dist/output.txt`

The retried upload and retried download restored the output file.

```
built
```

## `vtt list-dir node_modules/.vite/task-cache --ext .tmp --recursive`

No temporary download file was left behind after the truncated attempt.

```
```
