# daemon_lifecycle

## `remote-cache-server start --base-path /projects/test`

```
Remote cache server started
```

## `remote-cache-server start`

**Exit code:** 1

```
Remote cache server already started (cache.url exists)
```

## `cbor-http POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''S'\'', "value": '\''first daemon'\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": null}}
```

## `remote-cache-server stop`

Connect before deleting cache.url, then wait for the connection to close before restarting.

```
Remote cache server stopped
```

## `remote-cache-server start`

```
Remote cache server started
```

## `cbor-http POST /fetch --cbor '{"key": '\''A'\'', "secondary_key": '\''S'\''}'`

A new daemon starts with empty state.

```
{"status": 404, "content_type": "text/plain; charset=utf-8", "body": "Entry not found"}
```

## `remote-cache-server stop`

```
Remote cache server stopped
```

## `remote-cache-server stop`

```
No remote cache endpoint found
```
