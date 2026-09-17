# daemon_lifecycle

## `node ../node_modules/vite-task-tools/src/remote-cache/cli.ts start --base-path /projects/test`

```
Remote cache server started
```

## `node ../node_modules/vite-task-tools/src/remote-cache/cli.ts start`

**Exit code:** 1

```
Remote cache server already started (cache.url exists)
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /store --form-cbor 'metadata={"key": '\''A'\'', "secondary_key": '\''S'\'', "value": '\''first daemon'\''}'`

```
{"status": 200, "content_type": "application/cbor", "body": {"blob_id": null}}
```

## `node ../node_modules/vite-task-tools/src/remote-cache/cli.ts stop`

Connect before deleting cache.url, then wait for the connection to close before restarting.

```
Remote cache server stopped
```

## `node ../node_modules/vite-task-tools/src/remote-cache/cli.ts start`

```
Remote cache server started
```

## `node ../node_modules/vite-task-tools/src/cbor-http.ts POST /fetch --cbor '{"key": '\''A'\'', "secondary_key": '\''S'\''}'`

A new daemon starts with empty state.

```
{"status": 404, "content_type": "text/plain; charset=utf-8", "body": "Entry not found"}
```

## `node ../node_modules/vite-task-tools/src/remote-cache/cli.ts stop`

```
Remote cache server stopped
```

## `node ../node_modules/vite-task-tools/src/remote-cache/cli.ts stop`

```
No remote cache endpoint found
```
