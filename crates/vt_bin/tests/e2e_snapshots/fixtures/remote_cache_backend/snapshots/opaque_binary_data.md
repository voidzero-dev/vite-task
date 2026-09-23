# opaque_binary_data

## `remote-cache-server start`

```
Remote cache server started
```

## `cbor-http POST /store --form-cbor 'metadata={"key": b64'\''AP+A'\'', "secondary_key": '\'''\'', "value": b64'\''AP+A'\''}' --form-data blob=b64'AP+A'`

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": "1"}}
```

## `cbor-http POST /fetch --cbor '{"key": b64'\''AP+A'\'', "secondary_key": '\'''\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "exact", "value": b64'AP+A', "blob_id": "1"}}
```

## `cbor-http POST /fetch --cbor '{"key": '\'''\'', "secondary_key": '\'''\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "fallback", "key": b64'AP+A'}}
```

## `cbor-http GET /blob/1`

```
{"status": 200, "content_type": "application/octet-stream", "body": b64'AP+A'}
```

## `cbor-http POST /store --form-cbor 'metadata={"key": '\'''\'', "secondary_key": '\'''\'', "value": '\'''\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": null}}
```

## `cbor-http POST /fetch --cbor '{"key": '\'''\'', "secondary_key": '\'''\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "exact", "value": '', "blob_id": null}}
```

## `remote-cache-server stop`

```
Remote cache server stopped
```
