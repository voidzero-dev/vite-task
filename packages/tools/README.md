This package provides Node.js dependencies and commands for the test suites.

## Remote cache backend

Run `pnpm install` at the repository root to install `remote-cache-server` and `cbor-http` into this package's `node_modules/.bin`. The workspace self-dependency makes pnpm link the package's own commands there. The E2E harness already includes that directory in PATH. Both commands run TypeScript directly using the Node version in `.node-version`.

```sh
remote-cache-server start
cbor-http POST /store --form-cbor "metadata={\"key\": 'A', \"secondary_key\": 'S', \"value\": 'record'}"
cbor-http POST /fetch --cbor "{\"key\": 'A', \"secondary_key\": 'S'}"
remote-cache-server stop
```

`start` creates `cache.url` exclusively in the current directory, refusing to start if it already exists. It launches an in-memory daemon on a free loopback port and fills the file with its endpoint before returning. Each E2E case has its own directory and server state.

`stop` connects to the endpoint, deletes `cache.url`, and reads until EOF or a connection reset confirms shutdown. The daemon watches the directory for file changes and closes its listener before closing active connections. Wait for `stop` to finish before restarting in the same directory. Deleting the file manually also requests shutdown, but a later `stop` cannot wait for the server because the endpoint is gone. The harness enforces step timeouts and skips remaining steps when one times out. The detached daemon exits after five minutes so it is cleaned up even if `stop` never runs. Use `--max-lifetime-ms` on `start` for longer tests.

Daemon output goes to `cache.log`, and startup failures include that log. The log stays in the test's directory.

The backend implements `POST /fetch`, `POST /store`, and `GET /blob/{blob_id}` from the [remote cache RFC](https://github.com/voidzero-dev/vite-task/blob/rfc-cloudflare-remote-cache/docs/rfcs/0001-remote-cache.md#4-http-api-mapping). Keys and values are opaque bytes. Blobs remain unchanged until the daemon exits. Blob IDs are sequential strings to keep snapshots deterministic. There is no authentication or persistent storage.

`start --base-path /projects/test` places the API under that path and includes it in `cache.url`. Requests are limited to 64 MiB by default; use `--max-request-bytes` to change the limit. Keys, values, and blobs have no separate length limits.

## CBOR HTTP client

`cbor-http METHOD PATH` resolves the path against `cache.url`. An absolute HTTP URL works without that file. Request bodies use [CBOR extended diagnostic notation (EDN)](https://www.rfc-editor.org/rfc/rfc8610.html#appendix-G): `"text"` is text, `'bytes'` is a UTF-8 byte string, and `b64'AP+A'` contains arbitrary binary bytes.

| Option                  | Body                                                      |
| ----------------------- | --------------------------------------------------------- |
| `--cbor EDN`            | Encode EDN as CBOR with `Content-Type: application/cbor`. |
| `--data EDN`            | Send an EDN byte string as raw bytes.                     |
| `--form-cbor NAME=EDN`  | Add a CBOR multipart part.                                |
| `--form-data NAME=EDN`  | Add an EDN byte string as a raw multipart part.           |
| `--form-file NAME=FILE` | Add file contents as a raw multipart part.                |
| `--content-type TYPE`   | Override the request content type.                        |

Multipart options can repeat and retain their order. Raw bodies and parts default to `application/octet-stream`. Parts have no filename, including binary metadata.

Every response prints one EDN map containing `status`, `content_type`, and `body`. CBOR is decoded, text becomes a text string, and other bodies become byte strings. Byte strings use readable UTF-8 when possible and base64 EDN otherwise. HTTP error responses exit successfully so snapshots can assert their status and body. Argument errors, transport failures, and invalid CBOR responses exit unsuccessfully.

```text
{"status": 200, "content_type": "application/cbor", "body": {"kind": "exact", "value": 'record', "blob_id": null}}
```

Run `pnpm --filter vite-task-tools check` for type checking and `pnpm --filter vite-task-tools test` for the EDN-formatting unit test. Run `cargo test -p vt_bin --test e2e_snapshots -- remote_cache_backend --ignored` for the backend snapshots.

The backend snapshots are skipped on Windows because the PTY launcher cannot execute pnpm command shims.
