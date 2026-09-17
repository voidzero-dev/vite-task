# invalid_requests

## `remote-cache-server start`

```
Remote cache server started
```

## `cbor-http GET /missing`

```
{"status": 404, "content_type": "text/plain; charset=utf-8", "body": "Route not found"}
```

## `cbor-http POST /fetch --data 'bad'`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Expected application/cbor"}
```

## `cbor-http POST /fetch --data 'bad' --content-type application/cbor`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Expected byte strings: key, secondary_key"}
```

## `cbor-http POST /fetch --cbor '{"key": "text", "secondary_key": '\'''\''}'`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Expected byte strings: key, secondary_key"}
```

## `cbor-http POST /fetch --cbor '{"key": '\'''\''}'`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Expected byte strings: key, secondary_key"}
```

## `cbor-http POST /fetch --cbor []`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Expected byte strings: key, secondary_key"}
```

## `cbor-http POST /fetch --cbor '{"key": '\'''\'', "key": '\''duplicate'\'', "secondary_key": '\'''\''}'`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Invalid CBOR"}
```

## `cbor-http POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''S'\'', "value": '\''original'\''}' --form-data 'blob='\''original archive'\'''`

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": "1"}}
```

## `cbor-http POST /store --cbor {}`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Expected multipart/form-data"}
```

## `cbor-http POST /store --form-data 'blob='\''missing metadata'\'''`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Missing metadata"}
```

## `cbor-http POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''T'\'', "value": "wrong type"}'`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Expected byte strings: key, secondary_key, value"}
```

## `cbor-http POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''T'\'', "value": '\''replacement'\''}' --form-cbor metadata={}`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Invalid multipart body"}
```

## `cbor-http POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''T'\'', "value": '\''replacement'\''}' --form-data blob='one' --form-data blob='two'`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Invalid multipart body"}
```

## `cbor-http POST /store --form-cbor metadata={} --form-data unexpected='part'`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Invalid multipart body"}
```

## `cbor-http POST /store --form-data 'metadata='\''wrong content type'\'''`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Invalid multipart body"}
```

## `cbor-http POST /store --data ''\''--boundary\r\nContent-Disposition: form-data; name="metadata"\r\nContent-Type: application/cbor\r\n\r\n'\''' --content-type 'multipart/form-data; boundary=boundary'`

```
{"status": 400, "content_type": "text/plain; charset=utf-8", "body": "Invalid multipart body"}
```

## `cbor-http POST /fetch --cbor '{"key": '\''A'\'', "secondary_key": '\''S'\''}'`

Failed stores did not replace the value or blob.

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "exact", "value": 'original', "blob_id": "1"}}
```

## `cbor-http GET /blob/1`

```
{"status": 200, "content_type": "application/octet-stream", "body": 'original archive'}
```

## `cbor-http POST /fetch --cbor '{"key": '\''B'\'', "secondary_key": '\''T'\''}'`

Failed stores did not publish a secondary association.

```
{"status": 404, "content_type": "text/plain; charset=utf-8", "body": "Entry not found"}
```

## `remote-cache-server stop`

```
Remote cache server stopped
```
