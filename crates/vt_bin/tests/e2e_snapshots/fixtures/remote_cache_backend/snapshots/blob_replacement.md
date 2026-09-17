# blob_replacement

## `node ../node_modules/vite-task-tools/src/remote-cache/cli.ts start`

```
Remote cache server started
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts GET /blob/missing`

```
{"status": 404, "content_type": "text/plain; charset=utf-8", "body": "Blob not found"}
```

## `vtt write-file archive.txt 'first archive'`

```
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''S'\'', "value": '\''first'\''}' --form-file blob=archive.txt`

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": "1"}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\''A'\'', "secondary_key": '\''S'\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "exact", "value": 'first', "blob_id": "1"}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts GET /blob/1`

```
{"status": 200, "content_type": "application/octet-stream", "body": 'first archive'}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-data 'blob='\''second archive'\''' --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''S'\'', "value": '\''second'\''}'`

Accept blob before metadata, with neither part supplying a filename.

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": "2"}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\''A'\'', "secondary_key": '\''S'\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "exact", "value": 'second', "blob_id": "2"}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts GET /blob/2`

```
{"status": 200, "content_type": "application/octet-stream", "body": 'second archive'}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts GET /blob/1`

Previously returned IDs retain their original bytes.

```
{"status": 200, "content_type": "application/octet-stream", "body": 'first archive'}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''S'\'', "value": '\''no archive'\''}'`

Omitting blob clears the association.

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": null}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\''A'\'', "secondary_key": '\''S'\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "exact", "value": 'no archive', "blob_id": null}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''S'\'', "value": '\''empty archive'\''}' --form-data blob=''`

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": "3"}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\''A'\'', "secondary_key": '\''S'\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "exact", "value": 'empty archive', "blob_id": "3"}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts GET /blob/3`

An empty blob has an ID and downloads as an empty byte string.

```
{"status": 200, "content_type": "application/octet-stream", "body": ''}
```

## `node ../node_modules/vite-task-tools/src/remote-cache/cli.ts stop`

```
Remote cache server stopped
```
