# Remote cache server API design

This API stores and retrieves entries by `key`, with a `secondary_key` for fallback lookup and an optional blob. It defines server-visible behavior. Authentication and server implementation are out of scope.

The server treats keys, values, and blobs as opaque bytes. It does not interpret their schema or format.

## Endpoint

The user configures a base URL that includes the storage namespace, such as `https://cache.example.com/projects/my-project`.

| Operation | Method | URL |
| --- | --- | --- |
| Fetch metadata | POST | `{endpoint}/fetch` |
| Download a blob | GET | `{endpoint}/download/{blob_id}` |
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

The server looks up `key` first. If no entry exists, it follows the association from `secondary_key` to a stored `key` and retrieves that entry. If neither lookup resolves to an entry, it returns `not_found`.

Fallback includes the stored `key` to identify which entry matched. Fetch does not modify entries or associations.

Fetch returns the value and `blob_id` without transferring blob bytes. Downloads use a separate request so callers can inspect an entry before retrieving its blob.

### Key and value encoding

In these requests and responses, CBOR encodes the outer map, including field names and `kind` as text strings. The `key`, `secondary_key`, and `value` fields carry binary data without base64 overhead. The server reads the map but treats those fields as opaque bytes.

The server compares keys by byte equality. Any schema version or compatibility information encoded in a key remains opaque to the server.

## Download

```http
GET {endpoint}/download/{blob_id}
```

The server returns HTTP 200 with `Content-Type: application/octet-stream` and raw blob bytes, or HTTP 404 if the blob is unavailable.

## Store

```http
POST {endpoint}/store
Content-Type: multipart/form-data; boundary=...
```

| Part | Required | Content type | Content |
| --- | --- | --- | --- |
| `metadata` | Yes | `application/cbor` | The map below |
| `blob` | No | `application/octet-stream` | Raw blob bytes |

Metadata:

```text
{ key: bytes, secondary_key: bytes, value: bytes }
```

Multipart separates metadata from blob bytes, allowing streaming uploads without requiring streaming support in the CBOR decoder.

The server returns HTTP 200 with `Content-Type: application/cbor`:

```text
{ blob_id: string | null }
```

Omitting `blob` returns `null`. A present, zero-byte blob receives a `blob_id`, distinguishing an empty blob from no blob.

## Storage semantics

- Store replaces the value and blob under `key`. Omitting `blob` removes the entry's previous `blob_id`.
- Store updates `secondary_key` to reference the supplied `key`. The entry it previously referenced remains available through its own `key`.
- A successful store makes the complete entry, association, and blob available together. Later requests can retrieve them unless another write or eviction intervenes.
- Concurrent stores may resolve in either order. The server must keep each value paired with the blob from the same store request.
- Repeating an identical store follows the same replacement rules and need not return the same `blob_id`.
- The server may evict entries and blobs. Download may return HTTP 404 after a successful fetch because eviction can happen between requests.

### How entries and associations work

The server stores each value under a `key`. A `secondary_key` references one `key` and provides a fallback when the requested entry is absent. Separating these mappings lets the server change the fallback target while keeping older entries available for exact lookup.

```text
Entries:      key → value
Associations: secondary_key → key
```

These are logical mappings, not a prescribed database structure. The explanation below omits blobs and uses `A`, `B`, and `S` as labels for opaque keys, and `VA` and `VB` as labels for opaque values.

#### Store writes an entry and an association

Each store request writes the value under `key` and sets `secondary_key` to reference that key. If either mapping already exists, the server replaces it.

For example, starting with empty storage, `store(key=A, secondary_key=S, value=VA)` creates:

```text
Entries:      A → VA
Associations: S → A
```

#### Fetch follows the association when no exact match exists

The server first searches Entries using `key`. If it finds no entry, it looks up `secondary_key` in Associations and retrieves the referenced entry. Fetch changes neither mapping.

With the state above, `fetch(key=B, secondary_key=S)` finds no entry `B`. The server follows `S → A` and returns:

```text
{ kind: "fallback", key: A, value: VA }
```

The response includes `A` because the returned value belongs to that key, rather than the requested key `B`.

#### Changing an association keeps the previous entry

Storing a different `key` with the same `secondary_key` changes the fallback target. It does not delete the entry previously referenced by that `secondary_key`.

Continuing the example, `store(key=B, secondary_key=S, value=VB)` produces:

```text
Entries:      A → VA, B → VB
Associations: S → B
```

Entry `A` remains available through exact lookup. The association now selects `B` for fallback.

#### Exact lookup takes precedence over the association

An association does not restrict which entries can be retrieved. Even when `S` references `B`, an exact lookup for `A` returns `VA`. Fetching `A` does not change `S → B`.

With the state above and no further stores or evictions:

| Request | Result |
| --- | --- |
| `fetch(key=A, secondary_key=S)` | Exact match: `VA` |
| `fetch(key=B, secondary_key=S)` | Exact match: `VB` |
| `fetch(key=C, secondary_key=S)`, where `C` is absent | Fallback match: `key=B`, `value=VB` |
| `fetch(key=C, secondary_key=T)`, where `C` and association `T` are absent | `not_found` |

Fallback follows one reference; it does not search other entries for a match. If the referenced entry is unavailable, fetch returns `not_found`, even if other entries remain in storage.

## Errors and limits

Error responses use `Content-Type: application/cbor`:

```text
{ code: string, message: string }
```

The `code` field is stable for programmatic handling. The `message` field provides a human-readable explanation.

| HTTP status | Code | Meaning |
| --- | --- | --- |
| 400 | `invalid_request` | Malformed request or invalid fields. |
| 404 | `blob_not_found` | Blob unavailable. |
| 413 | `payload_too_large` | Request exceeds server size limits. |
| 500 | `internal_error` | Server could not complete the operation. |
| 503 | `unavailable` | Service temporarily unavailable. |

Fetch returns HTTP 200 with `kind: "not_found"` when neither lookup finds an entry because absence is an expected lookup result. Servers document their request-size limits.
