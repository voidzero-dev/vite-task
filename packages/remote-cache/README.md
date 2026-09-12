# Public remote cache service

This package implements the server in [RFC #716](https://github.com/voidzero-dev/vite-task/pull/716): a TypeScript Worker, primary D1 metadata, and a private R2 Standard bucket. Anyone can read an enabled namespace. Only a signed GitHub Actions token for the registered public repository's main-branch `push` job can publish.

The package contains no `vp run` client adapter. Cache keys, values, and blobs remain opaque. A successful lookup does not prove that a result is reusable; a client must validate its inputs and output archive.

## Protocol

The endpoint is `https://<host>/projects/<namespace>`. Namespaces use 1–63 lowercase letters, digits, or hyphens, starting with a letter or digit. An operator can register up to 100 namespaces per deployment.

| Request                                                                   | Success                                                                                                                                       |
| ------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| `POST /fetch`, `application/cbor`, `{ key: bytes, secondary_key: bytes }` | CBOR `{ kind: "exact", value: bytes, blob_id: string \| null }`, or `{ kind: "fallback", key: bytes, value: bytes, blob_id: string \| null }` |
| `GET /blob/{blob_id}`                                                     | Raw bytes with `application/octet-stream`                                                                                                     |
| `POST /store`, `multipart/form-data`                                      | CBOR `{ blob_id: string \| null }`                                                                                                            |

`/store` requires one `metadata` part with content type `application/cbor` and fields `{ key: bytes, secondary_key: bytes, value: bytes }`. An optional `blob` part has content type `application/octet-stream`. Either part order works. An omitted blob returns `null`; an empty blob gets an ID. Duplicate parts, unknown parts, duplicate envelope fields, extra fields, invalid types, and truncated bodies are rejected.

Empty and non-UTF-8 keys work. CBOR definite and indefinite maps and byte strings work, including noncanonical lengths. The bounded envelope decoder supports only the protocol's map and string types. It never decodes `value` contents. Chunked strings have a 4,096-chunk bookkeeping limit.

Fetch gives exact matches priority. A fallback follows the latest secondary-key association. A store replaces both mappings atomically. Reassigning a secondary key does not delete its former target. Replacing an entry changes the value seen through every association to that entry.

**Draft contract difference:** this implementation follows RFC #716 and returns `404` for a fetch miss. The still-open API draft [#713 at `362f5bd9`](https://github.com/voidzero-dev/vite-task/blob/362f5bd91bb32806b512d3d9a5339ff435bf5f0a/docs/remote-cache-server-api.md) instead specifies HTTP `200` with `kind: "not_found"`. Reconcile the drafts before releasing a client against this service.

Errors have `Content-Type: text/plain; charset=utf-8`. Codes are `400` for invalid input, `401` for invalid/missing/expired tokens, `403` for a signed token that fails write policy, `404` for absent data or unavailable namespaces/routes, `413` for size limits, `429` for admission limits, `500` for an incomplete operation, and `503` for unavailable authorization/storage, failed publication guards, quotas, or concurrency admission. `429` and `503` include `Retry-After: 60`.

All responses disable caching. No CDN cache, R2 public URL, presigned URL, administrative HTTP route, or authentication redirect is exposed. The request host is never used as the JWT audience.

## Local checks

Use Node.js 22.12 or newer and the repository's pinned pnpm version. From the repository root:

```sh
pnpm install --frozen-lockfile
pnpm check-remote-cache
```

`just remote-cache` runs the same checks. The command generates binding/runtime types, checks all source, operator, benchmark, and test files, runs isolated workerd/D1/R2 tests, and bundles a deployment dry run. Tests generate a temporary RSA key and intercept only GitHub's fixed JWKS URL. They need no Cloudflare account, GitHub credentials, or client adapter. CI runs them on Linux, macOS, and Windows.

From this package directory, `pnpm dev` starts Wrangler with local bindings. Apply the local schema with `pnpm exec wrangler d1 migrations apply INDEX --local`. No namespace is enabled in the template. Use `pnpm test` for a fully initialized, isolated smoke test; it exercises authorized writes without creating a development signing-key bypass in production.

## Setup

For automatic PR previews, main-branch staging deployments, and tests against real Cloudflare resources, follow the [deployment and e2e plan](docs/e2e-plan.md). It includes GitHub environment configuration, the complete test matrix, manual verification, and preview teardown.

Use a dedicated Worker, D1 database, and bucket. The operator requires `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_API_TOKEN` in its environment. The token needs account permissions for Workers Scripts, D1, and Workers R2 Storage, plus route/zone permissions if using a custom domain. Enable R2 in the account first. Credentials stay in the operator process and Wrangler; they are never stored in namespace policy or passed as command arguments.

Run from this package directory:

```sh
pnpm operator setup --name my-public-cache --namespace docs --repo owner/repository --origin https://my-public-cache.account-subdomain.workers.dev
```

Use the account's actual Workers subdomain. For a custom domain, set `--origin https://cache.example.com`. Setup disables the unused `workers.dev` alias for a custom domain and disables preview URLs. It rejects an R2 bucket with public custom domains and disables its `r2.dev` access.

Setup discovers resources by name, creates missing resources, applies migrations, resolves public repository and owner IDs through GitHub, records the default branch and exact audience, installs lifecycle/Cron settings, deploys, and prints the endpoint. Repeated setup does not duplicate resources or re-enable a withdrawn namespace. Reusing an existing namespace for a different repository is rejected. Configuration is written to the ignored `wrangler.operator.json`; keep a secure backup of this non-secret file.

The default `free` **resource profile** retains the RFC's 8 GB budget, 20,000 entries, 20,000 associations, and 16 generations per Cron run. It is not proof of compatibility with the Workers Free CPU limit. **Use Workers Paid for the full payload limits until production CPU measurements establish a supported Free profile.** Local measurements already exceed 10 ms in several cases. Setup does not purchase or change a Workers subscription.

`--profile paid` selects 256 generations per Cron run. It does not increase storage or retention. Select those independently with `--byte-limit` and `--retention-days`. Retention can be 1–365 days. For example:

```sh
pnpm operator setup --name my-public-cache --namespace docs --repo owner/repository --origin https://cache.example.com --profile paid --retention-days 30 --byte-limit 30000000000
```

Set GitHub Actions `id-token: write` only on the trusted publishing job. The token audience must equal the printed endpoint. No cache secret is needed in GitHub. A token with a customized `sub` can work because policy uses signed IDs, branch, visibility, and event claims. Tokens for fork repositories, tags, PRs, `pull_request_target`, or `workflow_run` are denied.

## Limits and authorization

| Resource                                     | Default maximum   |
| -------------------------------------------- | ----------------- |
| Each key                                     | 16 KiB            |
| Opaque value                                 | 4 MiB             |
| Store metadata                               | 5 MiB             |
| Fetch request                                | 40 KiB            |
| Blob                                         | 64 MiB            |
| Entire store request                         | 72 MiB            |
| Multipart headers per part                   | 8 KiB             |
| Multipart parts                              | 2                 |
| R2 upload part                               | 5 MiB, sequential |
| Store request deadline                       | 2 minutes         |
| Metadata / blob lookup deadline              | 15 seconds        |
| Upload lease                                 | 15 minutes        |
| Replacement/late-upload grace                | 10 minutes        |
| Concurrent buffered stores/reads per isolate | 2 / 4             |

The `LIMITS` Wrangler variable is a JSON object with optional keys `key`, `value`, `metadata`, `fetch`, `blob`, `store`, `headers`, and `deadlineMs`. It can lower the tested ceilings. Envelope limits must accommodate their field limits and framing. Raising ceilings requires code changes and new memory, CPU, and account-limit measurements. An HTTP request without `Content-Length` reserves the full store limit. Actual bytes replace that reservation only at publication; pending, retired, and deleting generations remain charged.

GitHub authorization uses `jose`, `RS256`, the fixed issuer `https://token.actions.githubusercontent.com`, and its fixed HTTPS JWKS endpoint. The server checks exact string types for audience, repository/owner IDs, visibility, branch, ref type, and event. It requires integer `exp`, `nbf`, and `iat`, permits 30 seconds of skew for not-before/issued-at checks, accepts at most a 15-minute token lifetime, and never publishes after expiry. Token headers cannot supply signing-key URLs or embedded keys.

Tokens are limited to 16 KiB. JWKS bodies are limited to 64 KiB, requests to five seconds, cached keys to ten minutes, and refresh attempts to one per 30 seconds per isolate, including failures. Unknown key IDs cannot cause unlimited refreshes. Missing usable keys fail closed. A maintainer's policy change increments its version; publication checks that version, current scope/deployment state, token expiry, lease, bytes, and identity quotas inside the D1 transaction.

The rate bindings apply before database or object access. Defaults are 600 requests/minute for each namespace's fetch/blob operation and 30 store attempts/minute. Invalid credentials consume store admission too. Unknown routes share catch-all identities. Limits are approximate per Cloudflare location; they are not a global billing cap. Reads do not write D1 counters or refresh retention.

## Operations

```sh
pnpm operator status
pnpm operator bind --namespace another-project --repo owner/another-repository
pnpm operator bind --namespace docs --repo owner/repository
pnpm operator policy --namespace docs --writes off
pnpm operator policy --namespace docs --enabled off
pnpm operator policy --namespace docs --enabled on --writes on
pnpm operator policy --namespace docs --retention-days 30 --byte-limit 30000000000
pnpm operator deployment --byte-limit 30000000000
pnpm operator deployment --writes off
pnpm operator deployment --enabled off
pnpm operator upgrade
```

`bind` refreshes repository display data, owner ID, and default branch for an existing repository, or registers a new namespace. It never silently moves existing public data to a different repository. Per-scope and deployment byte/entry/association limits all apply; use `--entry-limit` and `--association-limit` to change them explicitly. Lowering a limit below current use blocks further publication until cleanup or an operator change restores capacity.

`status` reports policy, charged bytes, entry/association counts, generation states, cleanup eligibility, and actual D1 storage. It warns at 400 MB. Set provider alerts at 80% of request, CPU, R2 operation/storage, and D1 read/write/storage allowances. Monitor cleanup delay and pending bytes, and pause writes before a backlog reaches the storage ceiling. Public requests and other services in the same account can exhaust allowances even when this cache's byte budget is respected.

Sampled structured logs include request ID, namespace, operation, exact/fallback/miss/error outcome, HTTP status, request/response sizes, duration, D1 rows/query latency/storage, and R2 operation attempts. Verified writes add signed repository ID, workflow ref, run ID/attempt, and commit SHA. They exclude tokens, keys, values, blobs, and database errors. `LOG_SAMPLE_RATE` defaults to `0.1` and applies to failures too. Provider transfer metrics account for disconnected downloads; logged response size describes the selected response. A server exact hit is distinct from a client-validated cache hit.

### Cleanup and withdrawal

Every five minutes Cron claims a bounded indexed set of eligible generations, aborts known multipart uploads, deletes immutable object names, then releases charges. Failed deletion is retried after ten minutes. Conditional claims and generation-specific foreign keys preserve concurrent replacement entries. An indexed cursor scans and deletes orphan associations in bounded batches.

Current entries expire seven days after commit by default. Replaced blob IDs retain their original bytes for ten minutes. Abandoned uploads get a late-operation grace period after their lease ends. R2 lifecycle rules abort unfinished multipart uploads after one day and expire objects after retention plus two days (9/32 days for 7/30-day retention). A retention high-water mark prevents later policy reductions from deleting older generations early. Lifecycle covers the crash window between multipart creation and recording its ID; it supplements D1 accounting.

To withdraw and delete one namespace:

```sh
pnpm operator purge --namespace docs --confirm docs
```

To delete all deployment data and resources:

```sh
pnpm operator teardown --confirm my-public-cache
```

Teardown first disables access and writes and schedules deletion. If generations remain, it exits with a message to check `status` and repeat after Cron drains them. It keeps Cron and accounting available until R2 accepts deletion of the empty bucket, then deletes D1 and the Worker. Lifecycle may need to finish orphan cleanup before the bucket is empty. These commands require the operator's Cloudflare credentials; no HTTP caller can administer the service.

Public data can include logs, source maps, and input metadata. Only publish results intended for public distribution. Making a GitHub repository private does not withdraw existing cache data; disable/purge its namespace. Disabling writes or changing policy stops pending publication at the D1 guard. Job cancellation alone does not revoke an already issued bearer token, and downloaded public copies cannot be recalled.

### Upgrade and recovery

Keep the lockfile and compatibility date pinned. `upgrade` applies additive migrations before deploying the Worker. Back up `wrangler.operator.json` and the `scopes`/`deployment` policy rows separately from disposable cache data. `status` exports readable policy data. Before a migration that changes the schema contract, retain a compatible Worker bundle; do not roll back to code that predates a required schema change.

A D1 restore cannot restore deleted R2 objects. Missing live values return `503`; missing blobs return `404`. After partial recovery, create a new namespace and retire the old one, or reconcile the generation records and objects before enabling it. Do not restore stale policy over an intentional withdrawal.

## Measurements and release checks

Run `pnpm benchmark` for isolated workerd profiling of cold/warm JWKS stores, 250 KB and 4 MiB values, and 5/20/50 MB plus 64 MiB blobs at concurrency one and two. It writes ignored `benchmark-results.json`. The initial run is in [measurements/local.json](measurements/local.json).

The benchmark records wall time, V8 CPU samples, and heap/backing-store observations. Codec encode/decode CPU is measured separately in Node. Sampling is diagnostic, excludes some native runtime work, and is not Cloudflare's CPU billing metric. Post-operation heap readings are not peak isolate memory. Maximum-sized concurrent requests also pass the local runtime's memory checks, but production CPU, memory, R2/D1 latency, and usage still require a limited trial before a Free-plan release claim. Use [Cloudflare's CPU profiling tools](https://developers.cloudflare.com/workers/observability/dev-tools/cpu-usage/) and deployed request metrics for that gate.

The tests cover protocol fixtures, every multipart split position, binary identity semantics, all write-policy claim classes, JWKS outages/cooldowns, no-blob/empty-blob behavior, atomic concurrent stores, failed publication guards, quota accounting, R2 failures, unknown-length cancellation, namespace withdrawal, and deletion retries. No platform is skipped. The Worker deployment template and operator commands are checked locally; actual resource provisioning requires an operator account.

Implementation references: [D1 transactions](https://developers.cloudflare.com/d1/worker-api/d1-database/), [R2 Worker API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/), [lifecycle rules](https://developers.cloudflare.com/r2/buckets/object-lifecycles/), and [rate-limit bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/).
