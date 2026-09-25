This package provides Node.js dependencies and commands for the test suites.

## Remote cache backend

Run `pnpm install` at the repository root to install `remote-cache-server` and `cbor-http` into this package's `node_modules/.bin`. The workspace self-dependency makes pnpm link the package's own commands there. The E2E harness already includes that directory in PATH. Both commands run TypeScript directly using the Node version in `.node-version`.

```sh
remote-cache-server cbor-http POST /store --form-cbor "metadata={\"key\": 'A', \"secondary_key\": 'S', \"value\": 'record'}"
remote-cache-server cbor-http POST /fetch --cbor "{\"key\": 'A', \"secondary_key\": 'S'}"
```

`remote-cache-server COMMAND [ARGS...]` starts the backend on a free loopback port and runs the command with `VP_REMOTE_CACHE_URL` set to the endpoint, `http://127.0.0.1:<port>/projects/test`. The fixed base path gives every endpoint a namespace path. The wrapper takes no options and passes all arguments to the command unchanged. The command inherits stdio. When it exits, the server stops and the wrapper exits with the command's exit code.

State persists in `remote-cache/` in the current directory, so consecutive commands share it. Each E2E case has its own directory and state. `state.json` holds the entries, associations, and next blob ID, with keys and values hex-encoded. Each blob is a file in `remote-cache/blobs/` named by its blob ID. Blob IDs are sequential strings and continue across invocations, keeping snapshots deterministic.

The backend implements `POST /fetch`, `POST /store`, and `GET /blob/{blob_id}` from the [remote cache server API](https://github.com/voidzero-dev/vite-task/pull/713). Keys, values, and blobs are opaque bytes without length limits. There is no authentication.

## CBOR HTTP client

`cbor-http METHOD PATH` appends the path to `VP_REMOTE_CACHE_URL`, so run it through `remote-cache-server`. Request bodies use [CBOR extended diagnostic notation (EDN)](https://www.rfc-editor.org/rfc/rfc8610.html#appendix-G): `"text"` is text, `'bytes'` is a UTF-8 byte string, and `b64'AP+A'` contains arbitrary binary bytes.

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
