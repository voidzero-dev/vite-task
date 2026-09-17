# request_limit

## `node ../node_modules/vite-task-tools/src/remote-cache/cli.ts start --max-request-bytes 1024`

```
Remote cache server started
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''S'\'', "value": '\''original'\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": null}}
```

## `node -e 'require('\''node:fs'\'').writeFileSync('\''large.bin'\'', Buffer.alloc(2048))'`

```
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''T'\'', "value": '\''replacement'\''}' --form-file blob=large.bin`

```
{"status": 413, "content_type": "text/plain; charset=utf-8", "body": "Request too large"}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\''A'\'', "secondary_key": '\''S'\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "exact", "value": 'original', "blob_id": null}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\''B'\'', "secondary_key": '\''T'\''}'`

```
{"status": 404, "content_type": "text/plain; charset=utf-8", "body": "Entry not found"}
```

## `node ../node_modules/vite-task-tools/src/remote-cache/cli.ts stop`

```
Remote cache server stopped
```
