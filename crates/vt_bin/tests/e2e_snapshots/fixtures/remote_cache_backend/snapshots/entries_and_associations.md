# entries_and_associations

## `node ../node_modules/vite-task-tools/src/remote-cache/cli.ts start`

```
Remote cache server started
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\''A'\'', "secondary_key": '\''S'\''}'`

```
{"status": 404, "content_type": "text/plain; charset=utf-8", "body": "Entry not found"}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''S'\'', "value": '\''VA'\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": null}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''U'\'', "value": '\''VA'\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": null}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-cbor 'metadata={"key": '\''B'\'', "secondary_key": '\''S'\'', "value": '\''VB'\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": null}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\''A'\'', "secondary_key": '\''S'\''}'`

The exact entry survives reassignment of S to B.

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "exact", "value": 'VA', "blob_id": null}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\''C'\'', "secondary_key": '\''S'\''}'`

The previous fetch did not change S. Fallback returns only B's key.

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "fallback", "key": 'B'}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''T'\'', "value": '\''VA2'\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": null}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\''A'\'', "secondary_key": '\''missing'\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "exact", "value": 'VA2', "blob_id": null}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\''C'\'', "secondary_key": '\''U'\''}'`

An older association to A still resolves after A is replaced.

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "fallback", "key": 'A'}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\''C'\'', "secondary_key": '\''T'\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "fallback", "key": 'A'}}
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\''C'\'', "secondary_key": '\''S'\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "fallback", "key": 'B'}}
```

## `node ../node_modules/vite-task-tools/src/remote-cache/cli.ts stop`

```
Remote cache server stopped
```
