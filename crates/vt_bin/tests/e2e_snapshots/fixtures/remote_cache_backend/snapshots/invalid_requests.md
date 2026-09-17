# invalid_requests

## `node ../node_modules/vite-task-tools/src/remote-cache/cli.ts start`

```
Remote cache server started
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts GET /missing`

```
{"status": 404, "content_type": "text/plain; charset=utf-8", "body": "Route not found"}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --data 'bad'`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Expected application/cbor"}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --data 'bad' --content-type application/cbor`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Expected byte strings: key, secondary_key"}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": "text", "secondary_key": '\'''\''}'`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Expected byte strings: key, secondary_key"}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\'''\''}'`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Expected byte strings: key, secondary_key"}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor []`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Expected byte strings: key, secondary_key"}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\'''\'', "key": '\''duplicate'\'', "secondary_key": '\'''\''}'`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Invalid CBOR"}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''S'\'', "value": '\''original'\''}' --form-data 'blob='\''original archive'\'''`

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": "1"}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --cbor {}`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Expected multipart/form-data"}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-data 'blob='\''missing metadata'\'''`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Missing metadata"}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''T'\'', "value": "wrong type"}'`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Expected byte strings: key, secondary_key, value"}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''T'\'', "value": '\''replacement'\''}' --form-cbor metadata={}`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Invalid multipart body"}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''T'\'', "value": '\''replacement'\''}' --form-data blob='one' --form-data blob='two'`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Invalid multipart body"}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-cbor metadata={} --form-data unexpected='part'`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Invalid multipart body"}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-data 'metadata='\''wrong content type'\'''`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Invalid multipart body"}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --data ''\''--boundary\r\nContent-Disposition: form-data; name="metadata"\r\nContent-Type: application/cbor\r\n\r\n'\''' --content-type 'multipart/form-data; boundary=boundary'`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Invalid multipart body"}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\''A'\'', "secondary_key": '\''S'\''}'`

Failed stores did not replace the value or blob.

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "exact", "value": 'original', "blob_id": "1"}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts GET /blob/1`

```
{"status": 200, "content_type": "application/octet-stream", "body": 'original archive'}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\''B'\'', "secondary_key": '\''T'\''}'`

Failed stores did not publish a secondary association.

```
{"status": 404, "content_type": "text/plain; charset=utf-8", "body": "Entry not found"}
```

## `node ../node_modules/vite-task-tools/src/remote-cache/cli.ts stop`

```
Remote cache server stopped
```
