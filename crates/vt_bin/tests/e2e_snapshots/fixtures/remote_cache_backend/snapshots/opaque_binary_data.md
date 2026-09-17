# opaque_binary_data

## `node ../node_modules/vite-task-tools/src/remote-cache/cli.ts start`

```
Remote cache server started
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-cbor 'metadata={"key": b64'\''AP+A'\'', "secondary_key": '\'''\'', "value": b64'\''AP+A'\''}' --form-data blob=b64'AP+A'`

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": "1"}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": b64'\''AP+A'\'', "secondary_key": '\'''\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "exact", "value": b64'AP+A', "blob_id": "1"}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\'''\'', "secondary_key": '\'''\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "fallback", "key": b64'AP+A'}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts GET /blob/1`

```
{"status": 200, "content_type": "application/octet-stream", "body": b64'AP+A'}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-cbor 'metadata={"key": '\'''\'', "secondary_key": '\'''\'', "value": '\'''\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": null}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\'''\'', "secondary_key": '\'''\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "exact", "value": '', "blob_id": null}}
```

## `node ../node_modules/vite-task-tools/src/remote-cache/cli.ts stop`

```
Remote cache server stopped
```
