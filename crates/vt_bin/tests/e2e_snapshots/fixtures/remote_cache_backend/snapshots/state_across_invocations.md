# state_across_invocations

## `remote-cache-server cbor-http POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''S'\'', "value": '\''VA'\''}' --form-data 'blob='\''first archive'\'''`

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": "1"}}
```

## `remote-cache-server cbor-http POST /fetch --cbor '{"key": '\''C'\'', "secondary_key": '\''S'\''}'`

A later invocation reads the stored entry and association.

```
{"status": 200, "content_type": "application/cbor", "body": {"kind": "fallback", "key": 'A', "value": 'VA', "blob_id": "1"}}
```

## `remote-cache-server cbor-http POST /store --form-cbor 'metadata={"key": '\''B'\'', "secondary_key": '\''T'\'', "value": '\''VB'\''}' --form-data 'blob='\''second archive'\'''`

Blob numbering continues across invocations.

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": "2"}}
```

## `vtt write-file remote-cache/blobs/1 'replaced archive'`

Each blob is a file named by its ID.

```
```

## `remote-cache-server cbor-http GET /blob/1`

```
{"status": 200, "content_type": "application/octet-stream", "body": 'replaced archive'}
```
