# Remote cache server API design

This API lets Vite+ clients share cached execution records and output archives. It defines the client/server contract. Authentication and server implementation are out of scope.

The server stores opaque keys, values, and attachments. The client handles wincode encoding, cache validation, and archive restoration. This keeps the server independent of the cache schema and archive format and lets the client reuse the existing [cache flow](fetch-validation-flow.md).

## Endpoint

The user configures a base URL that includes the storage namespace, such as `https://cache.example.com/projects/my-project`.

| Operation | Method | URL |
| --- | --- | --- |
| Fetch metadata | POST | `{endpoint}/fetch` |
| Download an attachment | GET | `{endpoint}/download/{blob_id}` |
| Store an entry | POST | `{endpoint}/store` |

## Fetch

```http
POST {endpoint}/fetch
Content-Type: application/cbor
```

The schemas describe field types, not literal request bodies. `bytes` means a CBOR byte string; `string` means a CBOR text string. All shown fields are required unless marked optional. Nullable fields must be present, using CBOR `null` when absent.

Request:

```text
{ key: bytes, secondary_key: bytes }
```

The server returns HTTP 200 with `Content-Type: application/cbor` and one of these responses.

Exact match:

```text
{ kind: "exact", value: bytes, blob_id: string | null }
```

Fallback match:

```text
{ kind: "fallback", key: bytes, value: bytes, blob_id: string | null }
```

No entry:

```text
{ kind: "not_found" }
```

The server checks `key` first. If no entry exists, it follows `secondary_key` to the associated primary entry. If neither lookup resolves to an entry, it returns `not_found`.

Fallback includes the stored primary key so the client can explain what changed. The client does not restore fallback results: the previous execution configuration differs, even if input hashes match.

For an exact match, the client validates listed input hashes, discovered path fingerprints, tracked environment values, and environment queries. It downloads the attachment only after validation passes. Separating metadata from the archive avoids transferring outputs the client cannot reuse.

### Key and value encoding

In these requests and responses, CBOR encodes the outer map, including field names and `kind` as text strings. The `key`, `secondary_key`, and `value` fields carry binary data without base64 overhead. The server reads the map but treats those fields as opaque bytes.

The client uses wincode to encode the contents of these binary fields. This is a client implementation detail. Both keys include the cache schema version and platform compatibility fields:

```text
key           = encode(schema version, platform, primary_key)
secondary_key = encode(schema version, platform, secondary_key)
```

The server only compares key bytes, so it needs no schema-version logic.

## Download

```http
GET {endpoint}/download/{blob_id}
```

The server returns HTTP 200 with `Content-Type: application/octet-stream` and raw attachment bytes, or HTTP 404 if the attachment is unavailable.

## Store

```http
POST {endpoint}/store
Content-Type: multipart/form-data; boundary=...
```

| Part | Required | Content type | Content |
| --- | --- | --- | --- |
| `metadata` | Yes | `application/cbor` | The map below |
| `blob` | No | `application/octet-stream` | Raw attachment bytes |

Metadata:

```text
{ key: bytes, secondary_key: bytes, value: bytes }
```

Why not encode attachment bytes in CBOR: the Multipart lets the client stream large archives without buffering the whole upload or depending on streaming support in its CBOR library.

The server returns HTTP 200 with `Content-Type: application/cbor`:

```text
{ blob_id: string | null }
```

Omitting `blob` returns `null`. This supports tasks with no output archive while preserving the distinction between an empty attachment and no attachment.

## Storage semantics

- Store replaces the value and attachment under `key`. Omitting `blob` clears the previous attachment association.
- Store points `secondary_key` to `key`. It keeps entries under other primary keys so clients can reuse them when switching back to an earlier configuration.
- A successful store makes the complete entry, secondary association, and attachment available together. Later requests can retrieve them unless another write or eviction intervenes.
- Concurrent stores may resolve in either order. The server must keep each value paired with its own attachment to prevent restoring mismatched outputs.
- Clients can retry an identical store. Retries follow the same replacement rules and need not return the same blob ID.
- The server may evict entries and attachments. Clients must handle HTTP 404 on download even after a successful fetch, since eviction can happen between requests.

## Errors and limits

Error responses use `Content-Type: application/cbor`:

```text
{ code: string, message: string }
```

Clients use stable error codes for handling failures and messages for human-readable explanations.

| HTTP status | Code | Meaning |
| --- | --- | --- |
| 400 | `invalid_request` | Malformed request or invalid fields. |
| 404 | `blob_not_found` | Attachment unavailable. |
| 413 | `payload_too_large` | Request exceeds server size limits. |
| 500 | `internal_error` | Server could not complete the operation. |
| 503 | `unavailable` | Service temporarily unavailable. |

Fetch misses return HTTP 200 with `kind: "not_found"` because absence is an expected lookup result. Servers document their request-size limits.

## Deferred decisions

The client policy for remote fetch remains open: fetch only when both local lookups find nothing, or after any local cache miss. Both policies use this API unchanged.

The client also defines the exact compatibility fields and wincode schema. HTTP protocol versioning is separate from cache schema versioning; the endpoint can include a protocol version path when needed.
