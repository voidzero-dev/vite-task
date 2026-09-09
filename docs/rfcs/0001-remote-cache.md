# RFC: Public remote cache for GitHub projects with `vp run`

Status: Draft design.

Updated: 2026-09-09. Repository baseline: `9a1d32cf`. API baseline: [PR #713](https://github.com/voidzero-dev/vite-task/pull/713), commit [`362f5bd9`](https://github.com/voidzero-dev/vite-task/blob/362f5bd91bb32806b512d3d9a5339ff435bf5f0a/docs/remote-cache-server-api.md). The API proposal remains a draft; check its final contract before implementation.

## 1. Motivation

Open-source maintainers should publish successful main-branch task results from GitHub Actions to a service in their own Cloudflare account. Developers and fork contributors should reuse these public results without signing in. The service needs no Vite+ hosted account or license service. Local caching remains the first tier; remote read failures fall back to task execution.

The current [docs deployment action](https://github.com/voidzero-dev/vite-plus/blob/ed1710f7aff8941436907a4d755d6b38c798ed41/.github/actions/deploy-docs/action.yml) transfers a whole task-cache directory through GitHub Actions Cache. A native remote cache can transfer one task's metadata and output blob, and make those results available to compatible developer machines.

This RFC implements PR #713 on Workers, D1, and R2, with anonymous reads and GitHub Actions OIDC authorization for writes. It also sets operational defaults and measures the conditions under which an individual or small team can stay within Cloudflare's free allowances.

## 2. Contract and scope

PR #713 owns the HTTP contract. This RFC owns the Cloudflare storage, authorization, limits, deployment, and cleanup choices. Version 1 requires public reads, repository-scoped write authorization, and namespace isolation for all three endpoints. Client fingerprint formats and cache-validation policy remain client responsibilities.

| Area           | Decision                                                                                                |
| -------------- | ------------------------------------------------------------------------------------------------------- |
| Hosting        | Open-source TypeScript Worker, private R2 Standard bucket, D1 database, five-minute Cron Trigger        |
| Endpoints      | `POST /fetch`, `GET /blob/{blob_id}`, `POST /store`, relative to a configured namespace endpoint        |
| Data           | CBOR envelope; opaque binary keys and values; optional opaque blob                                      |
| Lookup         | Exact key first, then one secondary-key association                                                     |
| Store          | Replace the entry and secondary association together after object storage succeeds                      |
| Access control | Anonymous reads; GitHub OIDC writes restricted to the registered repository and main-branch push events |
| Transfer       | One multipart HTTP store request; internal R2 multipart upload for larger blobs                         |
| Defaults       | Seven-day retention, 8 GB total R2 budget, 64 MiB maximum blob                                          |
| Failure        | Bounded waits; read failures become misses; explicit push reports publication failures                  |

The first delivery includes a self-deployment template and a native Rust client adapter. Cross-platform client behavior must work on macOS, Linux, and Windows. Reuse between different OS/architecture combinations requires a separate client compatibility agreement. Remote execution, a hosted SaaS, anonymous writes, a web dashboard, and cross-project deduplication are outside this plan. Private caches and Cloudflare One authorization are version 2 work.

## 3. Relationship to the current local cache

The current engine already separates exact lookup from a task association used to explain misses:

| Source evidence                                                                                                                                                               | Client integration consequence                                                         |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| [`ExecutionCache::try_hit` and `CacheEntryKey`](../../crates/vt/src/session/cache/mod.rs) use the spawn fingerprint and resolved input/output configuration for exact lookup. | Construct `key` before execution. Input changes can replace the value at the same key. |
| [`ExecutionCacheKey`](../../crates/vt_plan/src/cache_metadata.rs) identifies the task for the diagnostic association.                                                         | Supply a corresponding `secondary_key`; a fallback result can explain what changed.    |
| [`PostRunFingerprint`](../../crates/vt/src/session/execute/fingerprint.rs) and explicit glob checks validate observed inputs.                                                 | An exact server response still needs local validation before reuse.                    |
| [`update_cache`](../../crates/vt/src/session/execute/cache_update.rs) rejects failed, cancelled, incompletely traced, or otherwise ineligible executions.                     | Apply the same eligibility checks before remote storage.                               |
| [`archive`](../../crates/vt/src/session/cache/archive.rs) and [`replay_cache_hit`](../../crates/vt/src/session/execute/mod.rs) handle output files and terminal replay.       | Import verified data through a bounded staging path before reporting a hit.            |

An exact response means that the server found identical key bytes. It does not establish that current input files, inferred dependencies, or tracked environment values match. In the current engine, fallback data explains a miss; the client does not reuse its outputs. Preserve that distinction when adding the remote adapter. An exact entry that fails validation is a cache miss.

The client must define a portable, versioned encoding for its opaque fields before cross-machine reuse ships. It must preserve negative file dependencies, directory observations, tracked environment queries, and explicit glob membership. Schema, toolchain, and platform compatibility belong in client-controlled identity or validation data. PR #713 does not prescribe that encoding, and the Worker must not decode it. Existing local schema `v18` or a serialized SQLite directory is not a portable wire-format agreement.

Client flow: validate local cache; on a miss call `/fetch`; validate an exact response; fetch its blob only if reuse is possible; restore and promote to local storage. A fallback or `not_found` response leads to execution. A successful eligible execution updates local storage only. `vp cache push` publishes selected results through `/store`. A local hit makes no remote request during `vp run`.

## 4. HTTP API mapping

All paths are relative to an endpoint such as `https://cache.example.com/projects/docs-trusted-v1`. The endpoint includes the namespace. The following notation describes CBOR fields: `bytes` is a binary byte string, `string` is text, and nullable fields must be present with CBOR `null` when absent.

### Fetch metadata

```text
POST {endpoint}/fetch
Content-Type: application/cbor

{ key: bytes, secondary_key: bytes }
```

Return HTTP `200`, `Content-Type: application/cbor`, with one of:

```text
{ kind: "exact", value: bytes, blob_id: string | null }
{ kind: "fallback", key: bytes, value: bytes, blob_id: string | null }
{ kind: "not_found" }
```

Check `key` first. If no live entry exists, resolve `secondary_key` to a stored key and check that entry. Include the stored key only in the fallback variant. If neither resolves, return `not_found`. Fetch does not change entries or associations.

### Download a blob

```text
GET {endpoint}/blob/{blob_id}
```

Return HTTP `200`, `Content-Type: application/octet-stream`, with the raw blob. An unavailable blob returns `404`. The blob ID is an opaque server-generated reference, scoped by the endpoint. It is not an R2 URL or an authorization credential.

### Store an entry

```text
POST {endpoint}/store
Content-Type: multipart/form-data; boundary=...

metadata part (required), Content-Type: application/cbor:
  { key: bytes, secondary_key: bytes, value: bytes }

blob part (optional), Content-Type: application/octet-stream:
  raw blob bytes
```

Accept either part order. Return HTTP `200`, `Content-Type: application/cbor`:

```text
{ blob_id: string | null }
```

Omitting the blob returns `null`. A present zero-byte blob receives a non-null ID and downloads as an empty body.

Each successful store replaces `entries[key]` and sets `associations[secondary_key] = key`. For example:

| Operation           | Entries afterward                               | Association afterward |
| ------------------- | ----------------------------------------------- | --------------------- |
| Store `(A, S, VA)`  | `A → VA`                                        | `S → A`               |
| Store `(B, S, VB)`  | `A → VA`, `B → VB`                              | `S → B`               |
| Fetch `(A, S)`      | Unchanged; returns exact `VA`                   | Still `S → B`         |
| Fetch `(C, S)`      | Unchanged; returns fallback key `B`, value `VB` | Still `S → B`         |
| Store `(A, T, VA2)` | `A → VA2`, `B → VB`                             | `S → B`, `T → A`      |

Other secondary keys that already point to `A` also resolve to `VA2`. Changing `S` does not evict entry `A`.

### Errors

API errors use `Content-Type: text/plain; charset=utf-8`. Clients use the status code, not the human-readable message, to classify them.

| Status | Meaning                                                                  |
| ------ | ------------------------------------------------------------------------ |
| `400`  | Malformed request or invalid field types                                 |
| `404`  | Blob unavailable                                                         |
| `413`  | Request exceeds configured size limits                                   |
| `500`  | Operation could not complete                                             |
| `503`  | Service temporarily unavailable, including exhausted application budgets |

Normal metadata absence returns `200` with `not_found`. Version 1 reads require no credentials. For `/store`, this deployment adds `401` for a missing, invalid, or expired JWT and `403` for a verified token that fails the namespace write policy. Return `503` if authorization cannot be established because D1 or required signing keys are unavailable. Keep these errors generic and plain-text; authentication is outside PR #713. Rate limiting can return `429` with `Retry-After`. Cloudflare may reject requests before Worker code runs, so clients must tolerate non-protocol error bodies and avoid authentication redirects.

## 5. Public reads, GitHub OIDC writes, and explicit publication

Version 1 serves public cache data for open-source repositories on GitHub.com. Anyone can call `/fetch` and `/blob/{blob_id}` without credentials. Only an authorized GitHub Actions job can call `/store`. Developers use the checked-in endpoint without login, secrets, or individual permission setup. Version 2 covers private projects with Cloudflare One authorization.

### Client configuration and `vp cache push`

```ts
export default {
  run: {
    remoteCache: {
      url: 'https://cache.example.com/projects/docs-trusted-v1',
    },
  },
};
```

An endpoint enables public remote reads during `vp run`. Successful eligible tasks save their results locally. Publication is explicit: `vp cache push` sends selected local results through one `/store` request per entry.

Use `VP_REMOTE_CACHE_URL` to override the endpoint on a host and `--no-remote-cache` to disable remote use for an invocation. Task-level `remoteCache: false` excludes both remote reads and publication, while retaining local caching. Existing `cache: false`, `--no-cache`, and tool-requested cache disabling also exclude results from publication. No endpoint means no remote reads; an explicit push without an endpoint reports a configuration error.

By default, push selects eligible results produced by the latest completed `vp run` invocation in the current workspace and CI job/commit. Starting a new run replaces that selection; a failed or cancelled run does not leave an older successful run selected. Do not upload the whole local cache, entries imported from remote, or restored caches from other jobs by default.

Record the selection locally and pin its exact metadata/archive snapshot until publication or bounded cleanup. The current [cache update](../../crates/vt/src/session/execute/cache_update.rs) archives outputs, while [entry replacement](../../crates/vt/src/session/cache/mod.rs) can remove the previous archive. A delayed push therefore needs a snapshot or lease; it must not rebuild the archive from the later working tree or silently upload a replacement entry. Missing or inconsistent snapshots fail publication without modifying remote mappings.

`vp cache push` reports published, skipped, and failed entries. A remote publication failure returns a nonzero exit status while preserving local results; CI can mark this cache-only step `continue-on-error`. Entries commit separately, so partial success is possible. Repeated pushes follow PR #713 replacement semantics, not exactly-once delivery. No eligible entries is a successful no-op and needs no OIDC token.

### One-time repository binding

At deployment, the maintainer selects a public GitHub repository. Setup resolves and records its immutable `repository_id` and `repository_owner_id`, binds them to the namespace, and sets the allowed branch to `refs/heads/main`. A project whose main branch has another name needs that one server-side adjustment. Repository names are display data; names alone must not authorize writes. Do not let the first incoming token claim an unregistered namespace.

Use the configured namespace endpoint, without a trailing slash, as the exact OIDC audience. Store it in the server policy and derive it from the configured client URL. An alias or namespace change needs a matching policy update; do not infer the expected audience from an untrusted Host header. A repository transfer requires maintainer review and an owner-ID update. No per-developer registration or reusable cache secret is required.

### GitHub Actions token acquisition

The publishing job grants `permissions: id-token: write`. This lets a job request an OIDC token; the Worker decides whether it grants write access. The native client uses GitHub's `ACTIONS_ID_TOKEN_REQUEST_URL` and `ACTIONS_ID_TOKEN_REQUEST_TOKEN` to request a token with the namespace audience. See GitHub's [OIDC workflow configuration](https://docs.github.com/en/actions/how-tos/secure-your-work/security-harden-deployments/oidc-in-cloud-providers).

Only `vp cache push` requests the token. Send the returned JWT as `Authorization: Bearer <github_oidc_token>` to `/store`; the Worker verifies it directly, without a custom token exchange endpoint. Keep it in process memory, reuse only while valid for the same audience, and obtain a fresh token before expiry. The runner's request token is only for GitHub's token endpoint and must never be sent to the cache Worker. Outside Actions, push reports that GitHub OIDC is unavailable.

Do not forward either token across redirects or write it to config, command arguments, output, or the cache. Strip the OIDC request variables and remote-cache controls from task child environments, fingerprints, runner-aware environment APIs, serialized plans, and debug output, including wildcard environment selection. This does not isolate a privileged job from other code running as the same OS user; publication jobs execute trusted main-branch code.

### Worker write policy

Before consuming a store body or reserving quota, verify the JWT signature with a maintained library such as [`jose`](https://github.com/panva/jose), which supports Workers and remote JWKS. Use GitHub's fixed [OIDC issuer metadata](https://token.actions.githubusercontent.com/.well-known/openid-configuration) and HTTPS JWKS endpoint; allow only its supported signing algorithm (`RS256` initially). Do not accept `none`, symmetric algorithms, token-supplied key URLs, or decoded claims without verification. Bound token size, JWKS response size, fetch time, cache lifetime, and refresh frequency; unknown key IDs must not trigger unlimited outbound requests. If no usable cached key exists and key retrieval fails, return `503`; do not skip verification.

After cryptographic verification, require these signed claims and the enabled namespace policy:

| Claim                   | Required value                                      |
| ----------------------- | --------------------------------------------------- |
| `iss`                   | `https://token.actions.githubusercontent.com`       |
| `aud`                   | Exact configured namespace endpoint                 |
| `repository_id`         | Namespace's registered repository ID                |
| `repository_owner_id`   | Namespace's registered owner ID                     |
| `repository_visibility` | `public`                                            |
| `ref` / `ref_type`      | Configured main-branch ref / `branch`               |
| `event_name`            | `push`                                              |
| `exp`, `nbf`, `iat`     | Present and valid under a bounded clock-skew policy |

These are this deployment's trust conditions over [GitHub's documented claims](https://docs.github.com/en/actions/reference/security/oidc). Require exact types and values. Do not authorize from `actor`, a repository URL supplied by the client, a branch environment variable, or `sub` substring matching. GitHub supports different subject formats; explicit repository IDs and branch/event claims avoid relying on one textual `sub` layout. No organization-wide or repository-wide write grant substitutes for the full predicate.

A `pull_request_target` job can run in the base repository's default-branch context. Therefore `ref=refs/heads/main` alone is insufficient: deny `pull_request`, `pull_request_target`, `workflow_run`, tags, non-main branches, and other event types in v1. Fork repositories have different IDs and cannot write to the upstream namespace, even from their own `main` branch. Reusable workflows must retain matching caller-repository, branch, and event claims; the callee's identity alone grants no permission. See GitHub's [workflow event behavior](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#pull_request_target).

| Caller                                                                            | `POST /fetch`                 | `GET /blob/{blob_id}`         | `POST /store` |
| --------------------------------------------------------------------------------- | ----------------------------- | ----------------------------- | ------------- |
| Anonymous developer or fork contributor                                           | Allow                         | Allow                         | `401`         |
| Registered repository, public, main-branch push, valid audience/token             | Allow                         | Allow                         | Allow         |
| Valid GitHub token with wrong repo, owner, visibility, branch, event, or audience | Allow                         | Allow                         | `403`         |
| Invalid, expired, or forged token                                                 | Allow without using the token | Allow without using the token | `401`         |

A local command cannot obtain write permission by setting environment variables. Unknown or disabled namespaces expose no cache data. Scope every exact/fallback/blob lookup and mutation; a blob ID requested through the wrong namespace returns `404`. This separation prevents mixed project results, not discovery by an authorized reader: all enabled v1 namespaces are public. Keep the R2 bucket private so reads pass through Worker routing, retention, and budgets. Apply the same store verifier on all exposed routes and aliases.

### Publication trust and revocation

A GitHub token proves the job's identity, not that uploaded bytes match its commit or contain no secrets. The trusted publishing workflow builds the triggering main-branch commit and selects only results intended for public release. Values, input metadata, terminal logs, source maps, and blobs are all public; tasks that use private inputs or produce sensitive output must opt out. The Worker treats them as opaque and cannot redact them. Client validation still determines whether a public result can be reused.

At publication, the guarded D1 transaction rechecks the token expiry against server time, the scope's enabled/write-enabled state and unchanged policy version, lease, and quotas. An expired token or changed policy leaves existing mappings unchanged and stages objects for cleanup. No GitHub API call belongs inside that transaction. Tokens are short-lived bearer credentials and can be reused within their validity period; v1 does not maintain a per-token revocation or single-use ledger. Cancelling a job is not immediate token revocation. The maintainer can disable writes or change the namespace policy in primary D1; disabling the whole scope also stops subsequent reads.

Making a repository private does not remove previously published cache data. Maintainers must disable the public namespace and remove its objects when withdrawing publication; downloaded copies cannot be recalled. Private-cache access control belongs to v2.

## 6. Cloudflare storage model

Use D1 for binary-key indexes and transactions, and R2 for opaque values and blobs. D1's [2 MB row limit](https://developers.cloudflare.com/d1/platform/limits/) makes storing potentially larger values inline unsuitable. Store even small values in R2 initially to keep one storage path.

Conceptual tables, with names subject to implementation review:

| Table          | Identity and contents                                                                                                                    |
| -------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| `scopes`       | Public endpoint, enabled/write-enabled state, GitHub repo/owner IDs, branch, OIDC audience, policy version, retention, budgets, counters |
| `entries`      | Primary key `(scope_id, key BLOB)`; current generation ID                                                                                |
| `associations` | Primary key `(scope_id, secondary_key BLOB)`; target key as BLOB                                                                         |
| `generations`  | Random generation ID; scope, R2 object keys, optional blob ID, actual sizes, state, lease/expiry/retirement timestamps                   |

Use bound binary parameters and byte equality. Accept empty and non-UTF-8 keys within the size limits. Do not stringify, normalize, or interpret them as hex hashes. Index exact lookups, secondary lookups, blob IDs, and cleanup eligibility. Limit associations separately: many secondary keys can point to one entry.

Each store gets a fresh generation. Its R2 value object and optional blob object have unique immutable object names under an internal scope/generation prefix. The D1 entry pointer is mutable. This prevents concurrent stores from mixing one execution's value with another execution's blob.

Generation states cover `uploading`, `ready`, `retired`, and `deleting`. Persist object names and an internal upload lease before R2 writes. For a multipart upload, also record its R2 upload ID for abort/recovery. These records are backend bookkeeping; clients never receive an upload-session API.

Keep D1 operations on the primary in version 1. Read replication without a cross-request consistency agreement could return old mappings after a successful store or scope disable. Version 1 serves public data through the Worker without an additional CDN cache. Revisit edge caching separately, including namespace withdrawal and expiry behavior.

## 7. Fetch and download implementation

After rate limiting, the enabled-public-scope check, and bounded CBOR decoding, use a single indexed D1 query to select the exact live entry, or its secondary fallback if exact is absent. Select the response kind, stored key, and generation references from one database snapshot. Do not perform independent exact/fallback reads that can observe different commits.

Read the selected generation's value from R2 and return the corresponding CBOR variant. Return `503` if D1 identifies a live generation but its value object is missing or unreadable; that is a storage failure, not a normal cache miss. Expired or deleted entries count as absent. The client still validates the exact value before downloading outputs.

For `/blob/{blob_id}`, check the enabled public scope without authentication, resolve the blob ID in D1, and stream the R2 object to the response. A blob ID from another scope must not expose data. Ready blobs remain available until expiry; replaced blobs remain available during the retirement grace period described below. Return `404` for unavailable IDs, including IDs whose R2 object is gone.

No read writes last-access timestamps, extends retention, changes associations, or creates per-request analytics rows. Fixed retention and a short replacement grace period let fetch and download remain read-only. A blob may expire between fetch and download; the client handles `404` as a miss and executes.

## 8. Store implementation and concurrency

1. Verify the GitHub OIDC JWT and the namespace's write policy from section 5, capture its policy version and token expiry, then reserve storage capacity in D1. Use a bounded `Content-Length` when supplied; reserve the total request limit otherwise. Do not require that header. Create the generation and a 15-minute internal lease. Count concurrent reservations against scope and deployment budgets.
2. Parse multipart input incrementally with backpressure. Bound headers, part count, metadata bytes, blob bytes, and total bytes. Reject duplicate metadata or blob parts, invalid types, missing metadata, and truncated bodies. Accept metadata-first and blob-first bodies. Do not buffer the whole request with `formData()` or `arrayBuffer()`.
3. Buffer the bounded metadata part, decode its outer CBOR map, and preserve its byte-string fields. Write `value` to its generation-specific R2 object. The Worker does not inspect nested client data.
4. For a blob up to 5 MiB, use one R2 PUT after buffering that bounded amount. For a larger blob, use internal R2 multipart upload with 5 MiB parts and a smaller final part. Upload one part at a time, releasing buffers as progress allows. A present empty blob still requires an R2 object. Cloudflare documents the [multipart minimum and API](https://developers.cloudflare.com/r2/objects/multipart-objects/).
5. Await completion of both R2 objects and the full multipart request, including its closing boundary. Record actual sizes. In one guarded D1 batch, verify token expiry against server time, the lease, captured scope and unchanged policy version, enabled/write-enabled state, and budgets; mark the generation ready; replace `entries[key]`; set `associations[secondary_key] = key`; retire the old generation of that same key; and reconcile reserved bytes with actual bytes.
6. Return the new blob ID, or `null`. Publication must finish before the response; `waitUntil()` is reserved for best-effort cleanup or observations.

D1 [batches provide transaction rollback on statement failure](https://developers.cloudflare.com/d1/worker-api/d1-database/#batch). A conditional update affecting zero rows is not itself a SQL failure. Guard all publication mutations with the same valid-generation condition, inspect their results, and ensure a failed guard cannot leave one mapping changed. Prove this with concurrent-store and lease-expiry tests.

Readers see either the previous complete entry or the new complete entry. Concurrent stores follow the order of successful D1 commits; the last commit determines each affected mapping. Reassigning a secondary key does not retire the different entry it previously referenced. Replacing an entry changes what all associations to that key resolve to.

If R2 or parsing fails before publication, leave existing mappings unchanged and clean up the staged generation. If the response is lost after commit, the client cannot know whether storage succeeded. A retry is another store and may return a different blob ID or overwrite a newer concurrent store. PR #713 supplies no idempotency key or exactly-once guarantee.

R2 multipart parts are an implementation detail inside one incoming HTTP request. A client cannot resume them after disconnecting. Cancellation, worker termination, and failure between creating an R2 multipart upload and recording its ID require cleanup and a bucket lifecycle backstop.

## 9. Size limits and runtime budgets

PR #713 defines no fixed maximum length for client-supplied keys, values, or blobs. Its [size guidance](https://github.com/voidzero-dev/vite-task/blob/362f5bd91bb32806b512d3d9a5339ff435bf5f0a/docs/remote-cache-server-api.md#sizes) permits servers to impose resource limits and return `413`. The following are configurable defaults for this deployment, not protocol-wide limits:

| Resource                                               | Initial limit         |
| ------------------------------------------------------ | --------------------- |
| Each `key` or `secondary_key`                          | 16 KiB                |
| Opaque `value`                                         | 4 MiB                 |
| Store metadata part                                    | 5 MiB                 |
| Fetch request body                                     | 40 KiB                |
| Blob                                                   | 64 MiB                |
| Entire store request, including multipart overhead     | 72 MiB                |
| Internal R2 part buffer                                | 5 MiB                 |
| Upload lease                                           | 15 minutes            |
| Store request deadline / client blob-download deadline | 2 minutes / 5 minutes |

Return `413` when a field or request exceeds its configured byte limit, including when streaming discovers the excess. Reserve `400` for malformed input. Do not truncate or transform opaque fields to make them fit. Publish the configured limits in the deployment guide; changes must keep individual fields, envelope sizes, total request size, and measured runtime budgets consistent. The client treats a rejected store as skipped remote publication and retains its local result.

Bound multipart headers and CBOR container depth before allocation; reject ambiguous duplicate envelope fields. Support valid CBOR byte strings without requiring a canonical encoding. Test streaming boundaries and unknown-length request bodies. Envelope limits do not authorize decoding the opaque `value`.

Cloudflare's [request-body limits](https://developers.cloudflare.com/workers/platform/limits/#request-limits) depend on the Cloudflare account plan: Free and Pro allow 100 MB, Business 200 MB. Buying Workers Paid alone does not raise the Free account's 100 MB body limit. The 72 MiB cap leaves space below that limit. Larger transfers require compatible field and request limits, account allowances, and measured Worker settings.

Workers provides 128 MB per isolate, shared by concurrent requests. Budget metadata, CBOR copies, stream buffers, and concurrency together. Streaming reduces memory use; it does not make multipart parsing constant-cost. Workers Free allows 10 ms CPU per HTTP or Cron invocation. Include JWT signature verification and key loading in store measurements, with both cold and warm JWKS caches. Measure CBOR decoding and fetch encoding with 250 KB values and values up to the configured 4 MiB maximum, independently of blob size. Measure stores at 5, 20, 50 MB and the configured maximum, including concurrent uploads. The number of output files does not bound input metadata size. A Free deployment is a release target only for the sizes that pass those measurements. Lower limits or select Workers Paid if the implementation cannot meet them.

Keep no more than six external connections open, with sequential R2 part uploads. Size SQL batches and cleanup work to the Free plan's per-invocation limits. Do not interpret an average CPU estimate in the cost table as proof that large stores fit Free.

## 10. Retention, quotas, and cleanup

Retain current entries for seven days from successful store commit by default. Replacing a key starts a new retention interval for its new generation. Fetch does not refresh it. An association follows its target entry's lifetime; changing an association does not shorten the old target's retention.

Retain a replaced generation's value and blob for ten minutes after replacement to cover an in-flight fetch/download sequence. The former blob ID continues to identify the former bytes during that grace period. It must never return the replacement blob.

The Free profile reserves 8 GB across live, pending, retired, and deleting objects, plus limits of 20,000 live entries and 20,000 associations. Reserve space for new associations and entries during publication; updates of existing identities do not consume new slots. Warn at 400 MB of actual D1 storage. Maximum-sized keys and many associations can exhaust the database before the entry count limit.

A five-minute Cron invocation claims a bounded batch of expired, retired, or abandoned generations in D1, then deletes their known R2 objects or aborts uploads, and finally releases charged bytes. Use generation IDs and conditional state transitions so GC cannot remove a replacement entry or an association whose target has been recreated. Prune dangling associations in bounded indexed batches. Keep deleting objects charged until deletion succeeds.

An expired upload lease prevents publication. Allow an additional cleanup grace period for late R2 operations, and retry deletion until the generation is gone. Configure [R2 lifecycle rules](https://developers.cloudflare.com/r2/buckets/object-lifecycles/) as a backstop: abort unfinished multipart uploads after one day; expire generation objects after nine days for seven-day retention, or 32 days for 30-day retention. Lifecycle age starts at object creation, so its margin must cover upload leases and retirement grace. Lifecycle deletion is asynchronous; it does not replace D1 accounting or prompt cleanup.

Start with a maximum of 16 generations per Free Cron run and 256 on Paid, subject to measured CPU, query, and subrequest limits. The theoretical Free ceiling is 4,608 generations/day; actual cleanup can be lower. Both overwritten and expired generations contribute to the backlog. Pause stores with `503` before cleanup lag threatens the byte budget. Normal cleanup does not need an R2 LIST per entry.

Public reads need resource limits before D1/R2 work. Use a [Workers rate-limiting binding](https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/) with bounded keys for namespace/operation and stricter unauthenticated store admission; return `429` with `Retry-After`. Configure a catch-all key for unknown paths so callers cannot create unlimited limiter identities. The binding is approximate and local to each Cloudflare location, not a global billing cap. Rejected requests still invoke the Worker. Track anonymous miss traffic and denied writes separately; public traffic can exhaust the Free daily allowance even when storage fits. Keep a deployment/namespace disable control and avoid per-read D1 counter writes.

Application budgets prevent ordinary storage growth beyond the configured allowance. They do not guarantee a zero bill under arbitrary traffic, shared-account usage, failed uploads, or delayed lifecycle cleanup. Alert at 80% of provider allowances; leave hard admission limits and cleanup headroom in place.

## 11. Failure handling and operations

The client preserves local results if remote reads, validation, downloads, or explicit publication fail. `vp run` falls back to execution after a failed read; `vp cache push` reports failures through its own exit status. Use short metadata deadlines, bounded transfer deadlines, cancellation, a small concurrency limit, and an invocation-wide circuit breaker after repeated failures. Authentication and size failures should produce one actionable diagnostic rather than repeated attempts for every task. The client may retry transient reads within its time budget. Retrying an uncertain store repeats its replacement semantics.

Before extracting a remote blob, the client must validate its own format, compatibility, integrity information, output paths, and decompression/file-count limits. Reject path traversal, absolute paths, unsafe links, and malformed archives. Stage restoration before terminal replay or local promotion. The Worker stores opaque bytes and cannot perform these task-specific checks.

Log request ID, scope, operation, status, bytes, duration, and error class. For verified writes, include repository ID, workflow ref, run ID/attempt, and commit SHA from signed claims. Reads have no authenticated principal; do not add an identity API call. Do not log credentials or opaque request contents. Observe exact/fallback/not-found rates, client-validated hits, transferred bytes, D1 rows and latency, R2 operations, pending bytes, and cleanup lag. Sample successful Worker logs and bound error logging; no central telemetry service or paid analytics dependency is required. Client hit metrics must distinguish an exact lookup from successful reuse.

The Worker verifies GitHub tokens on writes and checks scope state in primary D1. Follow the policy-change and token-expiry semantics in section 5. Back up repository bindings and namespace policy separately from disposable cache data. D1 restoration does not restore deleted R2 objects; reconcile references or create a fresh namespace after partial recovery. Use additive migrations and document rollback compatibility.

## 12. Self-deployment and GitHub Actions migration

Deliver `packages/remote-cache` with TypeScript sources, pinned dependencies and lockfile, `wrangler.jsonc`, D1 migrations, protocol fixtures, and an operator CLI/guide. Setup creates a private R2 Standard bucket and D1 database, binds them as `ARTIFACTS` and `INDEX`, and installs lifecycle/Cron settings. The maintainer supplies the public GitHub repository; setup resolves its IDs through the [GitHub repository API](https://docs.github.com/en/rest/repos/repos#get-a-repository), stores its namespace write policy, and prints the public endpoint. Namespace creation remains an operator action, not open registration.

Deploy to `workers.dev` or an optional custom domain. Every exposed route must permit public reads and enforce the same GitHub JWT policy for stores; disable unused aliases. Keep administration behind the operator's Cloudflare credentials. Provide idempotent setup, policy changes, write/scope disable, upgrades, isolated smoke tests, and explicit teardown of stored data and Worker resources. Pin tested JWT-library, tooling, and compatibility versions. No cache secret needs to be generated or added to GitHub.

Default to the Free profile in section 10. The Paid profile changes operational budgets only after the operator selects them. Increasing retention or R2 storage remains an explicit choice, independent of the Workers subscription.

The following workflow excerpt shows the client flow. Retain the existing checkout, Vite+ setup, and dependency-installation steps. The endpoint can be checked into `vite.config.*`; the example uses a non-secret repository variable as a protected CI override. The build saves local results, then a separate step publishes them:

```yaml
on:
  push:
    branches: [main]

jobs:
  build:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      id-token: write
    env:
      VP_REMOTE_CACHE_URL: ${{ vars.VP_REMOTE_CACHE_URL }}
    steps:
      # Existing checkout, Vite+ setup, and dependency installation steps.
      - run: vp run build
        working-directory: docs
        env:
          DOCS_SITE_ORIGIN: ${{ vars.DOCS_SITE_ORIGIN }}
      - run: vp cache push
        working-directory: docs
        if: ${{ success() && github.event_name == 'push' && github.ref == 'refs/heads/main' }}
        continue-on-error: true
```

The workflow trigger and step condition avoid unnecessary upload attempts; the Worker independently checks the signed repository, branch, and event claims. A PR workflow uses the same public endpoint for reads, without `id-token: write` or a push step. Preserve the existing [`DOCS_SITE_ORIGIN` input tracking](https://github.com/voidzero-dev/vite-plus/blob/ed1710f7aff8941436907a4d755d6b38c798ed41/docs/vite.config.ts) and the workflow's configured site-origin value. Keep dependency installation and package-manager caching.

During a canary, retain existing task-directory restore/save within its trust boundary, but do not republish restored entries by default. Remove those steps after compatible clients pass anonymous fresh-checkout reuse and explicit-publication tests. Roll back publication by removing the push step or disabling writes on the server; disable remote reads by removing the endpoint or using `--no-remote-cache`.

## 13. Free and Paid capacity comparison

Prices below are USD before tax, checked on 2026-09-09. Estimates use a 30-day month and decimal MB/GB. Allowances assume this deployment is the account's only consumer. Workloads are illustrative; the frontend samples establish sizes, not typical user traffic.

### Provider allowances

A Cloudflare account plan, Workers Free/Paid, and R2 billing are separate choices. The service can use `workers.dev` without a Pro website plan. Users must [enable R2](https://developers.cloudflare.com/r2/get-started/); R2 overages can incur charges while Workers remains Free. Upgrading Workers does not increase R2's free allowance. See Cloudflare's [billing model](https://developers.cloudflare.com/billing/understand/how-billing-works/).

| Workers resource     | Free             | Paid Standard                                                                                                   |
| -------------------- | ---------------- | --------------------------------------------------------------------------------------------------------------- |
| Subscription         | $0               | $5/month minimum                                                                                                |
| Dynamic requests     | 100,000/day      | 10 million/month included; then $0.30/million                                                                   |
| HTTP CPU             | 10 ms/invocation | 30 million CPU ms/month included; then $0.02/million CPU ms; 30 s default per invocation, configurable to 5 min |
| Five-minute Cron CPU | 10 ms/invocation | 30 s/invocation                                                                                                 |
| Memory               | 128 MB/isolate   | 128 MB/isolate                                                                                                  |

Sources: [Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/) and [limits](https://developers.cloudflare.com/workers/platform/limits/). Storage/network wait time does not consume Worker CPU. Free request allowance is daily, and CPU eligibility applies to individual operations.

| D1 resource                   | Free                          | Paid Standard                                      |
| ----------------------------- | ----------------------------- | -------------------------------------------------- |
| Rows read                     | 5 million/day                 | 25 billion/month; then $0.001/million              |
| Rows written                  | 100,000/day                   | 50 million/month; then $1/million                  |
| Storage                       | 5 GB/account; 500 MB/database | 5 GB included, then $0.75/GB-month; 10 GB/database |
| Queries per Worker invocation | 50                            | 1,000                                              |

Sources: [D1 pricing](https://developers.cloudflare.com/d1/platform/pricing/) and [limits](https://developers.cloudflare.com/d1/platform/limits/). Count index maintenance and deletion as writes. Account-wide Free daily exhaustion interrupts database work until reset.

| R2 Standard resource            | Included with either Workers plan | Overage         |
| ------------------------------- | --------------------------------- | --------------- |
| Storage                         | 10 GB-month/month                 | $0.015/GB-month |
| Class A                         | 1 million/month                   | $4.50/million   |
| Class B                         | 10 million/month                  | $0.36/million   |
| Egress, DELETE, multipart abort | Free                              | Free            |

[R2 pricing](https://developers.cloudflare.com/r2/pricing/) counts multipart creation, each part, completion, and PUT as Class A; GET/HEAD as Class B. Storage billing averages daily peaks. Billable units round up to whole GB-months and million-operation units. Include unfinished and retired objects in observed peaks.

### Version 1 authorization cost

Public reads require no identity subscription. GitHub issues publishing-job tokens. GitHub Actions compute billing is separate from this cache estimate. JWT checks run in the Worker, and bounded JWKS refreshes add subrequests and latency. Benchmark both cold and warm verification with store parsing before claiming Workers Free eligibility.

### Usage model

Let `L` be daily public fetches after local misses, `F` fetches returning a value, `H` blob downloads after successful client validation, and `P` successfully published entries from explicit main-branch CI pushes, including overwrites. `P` counts entries, not command invocations, and is independent of developer misses. GitHub token requests are outside cache-Worker request counts; JWT verification and JWKS fetching belong in measured Worker CPU/subrequest budgets. Let `B` be mean blob bytes, `V` mean value bytes, `S = (B + V) / 1,000,000`, `E` live exact keys, and `R` retention days.

Each store takes one Worker request. Each metadata fetch takes one request, followed by one blob request only when needed. The Worker also reads the opaque value from R2. Internal R2 multipart upload changes storage operations, not client request counts:

```text
A per store = 1                         # no blob: value PUT
            = 2                         # blob <= 5 MiB: value PUT + blob PUT
            = 3 + ceil(B / 5 MiB)       # larger: value PUT + create + parts + complete

Worker invocations/day = ceil(1.10 * (L + H + P)) + 300
R2 Class A/month       = ceil(30 * 1.10 * P * A)
R2 Class B/month       = ceil(30 * 1.10 * (F + H))
Current R2 GB          = E * S / 1000
Total R2 GB            = current + pending + retired + awaiting deletion
```

Use per-size buckets or sum per-store operations for a mixed workload; `ceil(mean size)` can undercount multipart operations. The 10% operation reserve covers ordinary retries and maintenance; 300 daily invocations cover 288 Cron runs and routine management. It is not a bound against outages or arbitrary traffic. R2 completion/PUT success precedes publication; no extra HEAD per object is planned.

Only when each store creates a different retained exact key at a steady rate does `E = P * R`, giving `P * S * R / 1000` GB. With repeated stores to one key, retain its current generation and a short retirement backlog. Input changes do not necessarily create a different exact key. Do not multiply all stores by seven days and describe the result as actual storage usage. Overwrites still consume Worker, R2, D1, and cleanup operations.

For planning, budget 64 D1 rows read per fetch/download cycle plus 32 per store, and 40 rows written per store over its full lifecycle. Include scope lookups, indexes, accounting, association replacement, and cleanup. GitHub token verification needs no D1 credential table; retain conservative row budgets for repository-policy checks until measurements justify reducing them. Add 10% plus 5,000 reads/1,000 writes per day for maintenance:

```text
D1 reads/day  = ceil(1.10 * (64 * L + 32 * P)) + 5000
D1 writes/day = ceil(1.10 * 40 * P) + 1000
D1 storage MB = E * 4096 / 1000000        # provisional, ordinary small keys
```

These are engineering budgets, not measured costs. The storage estimate assumes roughly one association and current generation per key; account for extra associations, pending/retired generations, and large keys separately. R2 part receipts need not create a D1 row per part. Confirm `rows_read`, `rows_written`, index plans, actual database bytes, and cleanup costs before claiming these capacities.

### Key and value size evidence

The [PR #713 size example](https://github.com/voidzero-dev/vite-task/blob/362f5bd91bb32806b512d3d9a5339ff435bf5f0a/docs/remote-cache-server-api.md#sizes) reports a build tracking about 4,200 input paths and producing eight output files:

| Field           |  Reported size |
| --------------- | -------------: |
| `key`           |     About 1 KB |
| `secondary_key` | About 50 bytes |
| `value`         |   About 250 KB |
| `blob`          |   About 250 KB |

This is an upstream-reported example, not a measurement from our frontend artifact study or a population average. The client can record many input paths and hashes even when a task produces few output files. Here the value is as large as the blob. Budget and measure them separately; the Worker continues to treat their contents as opaque bytes.

For planning, interpret the approximate KB values as decimal: `V = 250,000` bytes and `B = 250,000` bytes give `S = 0.5 MB`. At 200 new distinct keys/day and seven-day retention, value plus blob storage is about 0.7 GB before staging and cleanup headroom. Both objects remain in R2. D1's provisional 4 KiB per-key estimate covers index and generation records only; validate it with the reported key sizes, including repeated key bytes in indexes and associations.

Every exact or fallback response transfers the value even if client validation prevents a blob download. Before protocol overhead and retries, daily response payload is `F * V + H * B` bytes. With 1,000 value-bearing fetches and 800 blob downloads at the example's sizes, that is about 450 MB/day, including 250 MB of values. This affects transfer time, CBOR work, and memory; request and R2 operation counts follow the existing equations.

### Artifact-size evidence

Use 5 MB for a small-output scenario. Measurements of four established products' published Vite frontend outputs on 2026-09-07 provide broader size evidence. The [artifact study](remote-cache-size-study/README.md) records pinned sources, exact bytes, and a reproduction script.

| Product/release                 | Output MB | `tar.zst` MB | `tar.zst` MB without source maps |
| ------------------------------- | --------: | -----------: | -------------------------------: |
| Directus `@directus/app@17.1.1` |     20.81 |         6.97 |                             6.97 |
| Docmost `v0.95.0`               |     14.83 |         4.77 |                             4.77 |
| Hoppscotch `2026.8.0`           |    127.53 |        32.18 |                            12.18 |
| n8n `n8n-editor-ui@2.16.2`      |    162.76 |        34.71 |                            13.15 |

These are official npm/Docker frontend outputs recompressed with zstd level 3, not local rebuilds or private cloud deployment measurements. They exclude backend tasks, dependencies, terminal events, and client validation metadata. Keep source maps when the task requires them. All four sampled archives fit the 64 MiB blob limit.

Three samples exceed 5 MB. Use 50 MB as an additional complete-result planning case for mature frontends, with room above these sampled archives. It is not a measured population average or an upper bound.

Measure one task at a time. A whole local-cache directory can contain several tasks and keys. During the canary, record compressed blob/value bytes, store and overwrite counts, live distinct keys, retention, pending peaks, task types, and lookup/validated-hit rates. Report mean, median, p95, maximum, and sample count by task type over at least two retention windows. Measure the fraction of sampled deployments that stay free before claiming support for most average users.

### Public-read workloads with explicit CI publication

In v1, many developer reads can share a small main-branch publication stream. The examples below use `H = 0.8 * L`, `F = L`, 5 MB per published entry, seven-day retention, and distinct keys for all stores. Publication counts are independent inputs:

| Public-cache workload | Fetches/day | Published entries/day | Current R2 | Worker/day | D1 reads/day | D1 writes/day | Free monthly estimate | Paid monthly estimate |
| --------------------- | ----------: | --------------------: | ---------: | ---------: | -----------: | ------------: | --------------------: | --------------------: |
| Public project        |       1,000 |                    20 |     0.7 GB |      2,302 |       76,104 |         1,880 |                    $0 |                 $5.00 |
| High read traffic     |      40,000 |                   100 |     3.5 GB |     79,610 |    2,824,520 |         5,400 |                    $0 |                 $5.00 |

Both fit the modeled request, row, storage, and R2-operation allowances, subject to per-request CPU qualification and operational headroom. The second case uses 2.3883 million Worker requests, 6,600 R2 Class A operations, and 2.376 million Class B operations per 30-day month. Under the same provisional 5 ms CPU assumption, Workers Paid stays at its $5 base charge. No identity-seat fee scales with the number of readers. These are workload examples, not measurements of typical projects or a guarantee against arbitrary public traffic.

### Illustrative workloads and prices

These scenarios compare storage and operation costs with `H = 0.8 * L`, `P = 0.2 * L`, and `F = L`. Only main-branch CI publications count toward `P`; the chosen ratio is a modeling assumption. Assume each store creates a distinct key. Set `V = 250 KB` (250,000 bytes), based on the upstream example, and include it in `S`. Calculate multipart counts from the remaining blob bytes. Average CPU of 5 ms per invocation is a Paid-cost assumption; Free eligibility requires per-request measurements.

| Workload             | Fetches/day | Stores/day | Mean size | Retention | Current R2 | Worker/day | D1 writes/day |
| -------------------- | ----------: | ---------: | --------: | --------: | ---------: | ---------: | ------------: |
| Individual           |         100 |         20 |      1 MB |    7 days |    0.14 GB |        520 |         1,880 |
| Small team           |         500 |        100 |      5 MB |    7 days |     3.5 GB |      1,400 |         5,400 |
| Active small team    |       1,000 |        200 |      5 MB |    7 days |       7 GB |      2,500 |         9,800 |
| Longer retention     |       1,000 |        200 |      5 MB |   30 days |      30 GB |      2,500 |         9,800 |
| Larger outputs       |       1,000 |        200 |     20 MB |    7 days |      28 GB |      2,500 |         9,800 |
| Mature frontend case |       1,000 |        200 |     50 MB |    7 days |      70 GB |      2,500 |         9,800 |
| Busy team            |      20,000 |      4,000 |      5 MB |    7 days |     140 GB |     44,300 |       177,000 |
| Large organization   |     100,000 |     20,000 |      5 MB |    7 days |     700 GB |    220,300 |       881,000 |

| Workload                                    | Workers Free + R2 monthly estimate                              | Workers Paid + R2/D1 monthly estimate |
| ------------------------------------------- | --------------------------------------------------------------- | ------------------------------------: |
| Individual / small team / active small team | $0 within modeled allowances                                    |                                 $5.00 |
| Longer retention                            | $0.30; requires higher storage budget                           |                                 $5.30 |
| Larger outputs                              | $0.27; requires higher storage budget                           |                                 $5.27 |
| Mature frontend case                        | $0.90; requires higher storage budget and Free CPU verification |                                 $5.90 |
| Busy team                                   | Does not fit D1 Free writes                                     |                                 $6.95 |
| Large organization                          | Exceeds Free requests, writes, and single-database storage      |                                $19.91 |

The public cache has no per-reader subscription charge. These steady-state prices exclude pending/retired storage, other account usage, domain costs, GitHub Actions compute, and unusual or abusive traffic. Free eligibility still requires CPU measurements, including JWT verification on stores. The default 8 GB profile refuses excess stores; it does not incur the higher-storage scenario by itself. R2 Class A remains within one million monthly operations in the 20/50 MB rows: 46,200/85,800 operations under the chosen multipart model.

For the large-organization row: 6.609 million Worker requests/month and 33.045 million CPU ms cost about $5.06; 700 GB R2 storage costs $10.35; 1.32 million Class A operations cost $4.50 after free allowance and unit rounding; 5.94 million Class B operations remain included. Modeled D1 use is 232.47 million reads, 26.43 million writes, and about 573 MB of current-key metadata, within Paid allowances. Cache infrastructure subtotal: about $19.91/month. Bursts, CPU outliers, and D1 throughput still need load tests; monthly allowances do not promise a request rate.

### How much can remain free?

For distinct keys, the 8 GB application budget gives these storage-only ceilings before pending bytes and cleanup headroom:

| Mean stored size | New distinct keys/day, 7 days | New distinct keys/day, 30 days |
| ---------------- | ----------------------------: | -----------------------------: |
| 1 MB             |                         1,142 |                            266 |
| 5 MB             |                           228 |                             53 |
| 20 MB            |                            57 |                             13 |
| 50 MB            |                            22 |                              5 |

Compute `floor(8000 / (S * R))`; other limits may bind first. If 200 daily stores repeatedly replace the same ten 50 MB keys, current storage is about 0.5 GB, plus temporary old generations and staging. If they create 200 different keys daily, seven-day current storage is 70 GB. The client's actual key reuse therefore matters as much as archive size.

In the stress comparison where `P = 0.2 * L`, with 80% blob downloads and no storage constraint, Free Worker requests allow about 45,318 fetches/day. The provisional D1 write budget binds earlier at 11,250 fetches/day, or about 8,977 at an 80% write alert threshold. Cleanup and CPU can reduce these numbers. For read-only workloads, D1 reads and Worker requests bind instead; apply the equations with `P = 0`.

The plan targets free operation through local-first public reads, one metadata fetch, blob downloads only after validation, explicit main-branch publication, replacement semantics, no D1 writes on reads, bounded retention, and private Standard R2. Read traffic can grow without adding writers or user-seat fees. The modeled examples fit free allowances only while traffic, storage, and per-request CPU remain within their budgets. Production measurements remain necessary to substantiate a claim about typical users.

## 14. Storage tradeoffs

D1 plus R2 preserves atomic replacement of two indexes while streaming object data. An R2-only implementation would need another coordination mechanism for those mappings. Workers KV would require a separate consistency design for replacement and scope-state changes. Durable Objects could serialize writes and accounting, but would add another state service; consider them only if D1 concurrency measurements show a need.

Storing small values inline in D1 could remove one R2 read/write for many tasks. Defer this optimization until value-size measurements justify a split storage path; values above D1's row limit would still need R2. Cross-result content deduplication could reduce repeated asset storage but adds reference accounting and changes garbage collection.

## 15. Delivery stages and acceptance

1. **Public reads and OIDC write policy.** Exercise every cell of section 5's permission matrix, including anonymous fresh-checkout reads. Test wrong issuer/audience, forged signatures, missing claims, expired/future tokens, wrong repository/owner/visibility, forks on their own main branch, PR and `pull_request_target` events, `workflow_run`, tags, non-main refs, and reusable workflows. Cover JWKS rotation, unknown-key refresh bounds, outages, policy changes, namespace disable, same-key/blob-ID isolation, and route/alias bypass attempts. Stores rejected at admission must reserve no quota and perform no R2 operation.
2. **Protocol and storage foundation.** Pin the final PR #713 contract; add CBOR/multipart fixtures and schema migrations. Test exact/fallback/not-found, required nullable fields, arbitrary binary/empty keys, replacement, and associations shared across keys. Add size fixtures for 1 KB keys, 50-byte secondary keys, and 250 KB values/blobs, plus configured maxima and just-over-limit `413` cases. Fetch must leave both mappings unchanged. Verify absent versus empty blobs and plain-text errors.
3. **Streaming and atomic publication.** Test both part orders, unknown content length, boundaries split across stream chunks, malformed/truncated bodies, maximum sizes, and cancellation. Exercise concurrent same-key and same-secondary-key stores, lease/token expiry, changed write policy before commit, lost responses, R2 failures, and rollback guards. No visible entry may reference an incomplete generation or change one mapping after a failed publication guard.
4. **Explicit client publication and canary.** Agree on the push command, selection semantics, and portable client encoding. Prove that `vp run` never stores remotely and only explicit push acquires OIDC credentials. Test snapshot pinning, local replacement/eviction, changed working-tree files, failed-run selection reset, opt-outs, exclusion of imported/restored entries, no-op push, partial failure, and retry behavior. On Unix and Windows, verify token acquisition/redaction, credential handling across redirects, exact validation, diagnostics-only fallback, inferred/missing inputs, tracked environment queries, safe restoration, local promotion, and failed-read fallback. Demonstrate anonymous reuse in a compatible fresh checkout after a main-branch push.
5. **Cleanup and self-deployment.** Prove concurrent quota reservation, old-blob grace, current-pointer protection during GC, bounded association cleanup, abandoned multipart cleanup, and lifecycle margins. Ship setup/policy-change/disable/upgrade/teardown guides and a real GitHub Actions-to-Worker smoke test. Test anonymous rate limits and namespace withdrawal; local emulation does not establish production OIDC or provider-limit behavior.
6. **Capacity qualification.** Measure operation counts, actual D1 storage, JWT/JWKS cold and warm costs, Free CPU across value/blob sizes, concurrent memory, and sustainable cleanup. Update cost tables with independent public-read and main-branch publication rates. Sample task mixes and key replacement over two retention windows before claiming broad free coverage.

## 16. Version 2 and implementation questions

Version 2 can add private projects using Cloudflare One: Access policies protect reads and writes, service tokens serve automation that cannot use GitHub OIDC, and Managed OAuth provides developer login. Retain namespace isolation and explicit publication. Separate private namespaces or deployments from public v1 endpoints; a private authorization failure must never fall back to public access. Revisit [Workers Access integration](https://developers.cloudflare.com/workers/configuration/cloudflare-access/), [Managed OAuth](https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/managed-oauth/), revocation semantics, and [Access pricing](https://www.cloudflare.com/sase/products/access/) when designing that version. None is a v1 dependency or cost.

Implementation questions include additional push selection flags, snapshot lifetime, portable client encoding and compatibility, JWT time tolerances and key-cache limits, measured Free CPU and D1 costs, and representative public traffic/publication rates. Verify compatibility with the merged version of PR #713 before release.
