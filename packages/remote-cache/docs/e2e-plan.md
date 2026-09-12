# Cloudflare deployment and e2e verification

This plan checks the service through its public HTTP endpoints after deployment to Cloudflare. A deployment dry run or a passing Miniflare test is not evidence of a successful Cloudflare deployment.

## Deployment flow

`.github/workflows/remote-cache-deploy.yml` runs when a PR or a push to `main` changes this package, its deployment workflows, or the root dependency/build configuration. It also supports manual runs from the default branch.

1. Check the exact source commit with `pnpm check-remote-cache`.
2. Create or update dedicated staging resources and apply D1 migrations.
3. Check that the R2 bucket has no public domain. Seed public test fixtures through the authenticated operator API.
4. Test the deployed HTTP endpoints. Every response must identify the expected commit and workflow attempt through `X-Remote-Cache-Deployment`.
5. Save a JSON report and manual fixtures as a seven-day workflow artifact.
6. Update one PR comment with the commit, result, endpoint, artifact link, and manual instructions. A failed or skipped deployment never gets a ready message.

Each PR uses `<prefix>-pr-<number>` for its Worker, database, and bucket. Main uses `<prefix>-main`. The default prefix is `vp-cache-ci`. Each deployment has three public namespaces: `e2e`, `other`, and `manual`. Resources contain synthetic data only. They are separate from production and from other PRs.

Deployment and teardown use the same concurrency group. They do not cancel a resource operation in progress. An obsolete PR commit cannot start deployment or replace the current PR comment. A later commit can replace a preview at the same URL; manual checks must compare the deployment ID from the saved manifest.

## GitHub and Cloudflare setup

Use a dedicated Cloudflare staging account or dedicated resources in an account that permits this CI workload. Select Workers Paid for the configured payload sizes. The `paid` operator profile controls cleanup batch size; it does not purchase a subscription.

Create the GitHub environment `remote-cache-staging` and add these environment secrets:

| Secret                  | Purpose                                                                      |
| ----------------------- | ---------------------------------------------------------------------------- |
| `CLOUDFLARE_ACCOUNT_ID` | Account that owns the staging resources                                      |
| `CLOUDFLARE_API_TOKEN`  | Account-scoped Workers Scripts, D1, and Workers R2 Storage write permissions |

Set these **repository variables**, which the notification job also needs:

| Variable                         | Value                                                                                                                           |
| -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| `REMOTE_CACHE_WORKERS_SUBDOMAIN` | Account subdomain only, for example `example` for `example.workers.dev`                                                         |
| `REMOTE_CACHE_RESOURCE_PREFIX`   | Optional; defaults to `vp-cache-ci`. A custom prefix must end in `-ci` and use at most 34 lowercase letters, digits, or hyphens |
| `REMOTE_CACHE_DEPLOY_ENABLED`    | Set to `true` after the environment and account are ready                                                                       |

Enable R2 in the account. The token must allow resource creation and deletion as well as data access. No custom domain or DNS permission is needed. Do not attach production bindings or secrets to these Workers. The GitHub environment can require a maintainer review if the repository needs one before a deployment.

The workflow exposes Cloudflare credentials only to deployment, verification, and cleanup commands. Dependency installation runs before those credentials enter the step environment. Internal PR contributors must be trusted to change deployment code. Fork and Dependabot PRs run local checks without Cloudflare credentials. Their check summary explains why no preview is available; they receive no deployment comment. Cleanup uses ordinary closed-PR events for internal branches and executes code from the default branch. To preview a fork change, a maintainer must copy the reviewed commit to a branch in this repository and open a PR.

Merge the workflow and cleanup script to the default branch before relying on automatic cleanup. GitHub can suppress `pull_request` workflows when a PR has merge conflicts. Use the manual cleanup workflow with the closed PR number if the close event does not run. Manual deployment dispatches use the default branch and cannot select an arbitrary PR revision.

## Authorization and test levels

The production write policy requires a real GitHub token with `event_name=push`, the registered repository and owner IDs, its configured main branch, and the exact namespace audience. A PR token or a `workflow_dispatch` token cannot pass this policy. GitHub signs the event claim; the test runner cannot change it.

| Level                          | Trigger                                     | What it proves                                                                                                             |
| ------------------------------ | ------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| Local regression               | Every normal CI run, on Linux/macOS/Windows | Protocol, storage state transitions, failures, and the e2e driver against workerd/D1/R2 emulators                          |
| PR Cloudflare verification     | Related changes in an internal PR           | Deployment revision, migrations, private R2, real public HTTP reads, real rejected OIDC writes, policy changes, and expiry |
| Main Cloudflare verification   | Related push to `main`                      | All PR cases plus real authorized HTTP writes, multipart uploads, concurrent maximum payloads, and the real Cron schedule  |
| Manual Cloudflare verification | Default-branch workflow dispatch            | Public reads and rejected writes; `full=true` also waits for real Cron cleanup                                             |
| Release / incident exercises   | Maintainer-controlled staging session       | Long-running limits, provider outages, lifecycle delays, restore, rollback, and real hostile workflow identities           |

The runner obtains tokens directly from GitHub with `id-token: write`. It checks the token request host, rejects redirects, bounds responses, and caches each audience separately for three minutes. Tokens remain in memory. Reports and artifacts contain no GitHub or Cloudflare credentials. The Worker has no alternate issuer, test signing key, administrative HTTP endpoint, or weakened write policy.

The operator seeds immutable objects and generation rows for read tests. This validates reads independently of upload authorization. Seeded fixtures do **not** count as successful `/store` coverage. Only main-branch push runs report authorized HTTP write coverage.

## Automated case matrix

“Local” refers to the existing regression suite and the new shared e2e-driver tests. “PR” and “Main” refer to real Cloudflare runs.

Status assertions follow the [local RFC](0001-remote-cache.md#4-http-api-mapping). Fetch misses and unavailable blobs return plain-text `404`. A missing or unreadable value for a live entry returns `503`.

| Case                                             | Local                         | PR                            | Main                      | Required result                                                                                                                  |
| ------------------------------------------------ | ----------------------------- | ----------------------------- | ------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| Exact source revision                            | Yes                           | Yes                           | Yes                       | Expected deployment ID on responses; fail if the URL serves another revision                                                     |
| Migrations and private R2                        | Yes                           | Yes                           | Yes                       | Setup succeeds, seeded data works, `r2.dev` is disabled, and custom R2 domains are absent                                        |
| Anonymous exact lookup                           | Yes                           | Yes                           | Yes                       | `200` CBOR with exact opaque value and matching blob ID                                                                          |
| Anonymous fallback                               | Yes                           | Yes                           | Yes                       | `200` CBOR with the associated stored key and matching value                                                                     |
| Read-only accounting                             | Yes                           | Yes                           | Yes                       | Reads do not change charged bytes or entry/association counts                                                                    |
| Missing entry/blob                               | Yes                           | Yes                           | Yes                       | Plain-text `404`, without redirects                                                                                              |
| Namespace isolation                              | Yes                           | Yes                           | Yes                       | Another namespace cannot resolve the key, association, or blob ID                                                                |
| Malformed / oversized fetch                      | Yes                           | Yes                           | Yes                       | `400` / `413`; no truncation                                                                                                     |
| Missing or forged token                          | Yes                           | Yes                           | Yes                       | `401` and no new generation                                                                                                      |
| Wrong audience                                   | Yes                           | Yes                           | Yes                       | A real signed token for another namespace gets `403`                                                                             |
| PR / dispatch token                              | Yes                           | Yes                           | Applicable on manual runs | `403` and no new generation                                                                                                      |
| Immediate scope withdrawal                       | Yes                           | Yes                           | Yes                       | Existing metadata/blob reads get `404` without redeployment; restore re-enables reads                                            |
| Missing live value / blob object                 | Yes                           | Yes                           | Yes                       | Value fetch gets `503`; blob download gets `404`; restore works                                                                  |
| Real authorized store                            | Emulated GitHub keys          | No                            | Yes                       | GitHub-signed main-branch push token permits `/store`                                                                            |
| Opaque / empty fields                            | Yes                           | Read fixtures                 | Yes                       | Preserve binary values; an omitted blob gets `null`, an empty blob gets an ID                                                    |
| Unknown request length                           | Yes                           | No                            | Yes                       | A streamed body without `Content-Length` succeeds within limits                                                                  |
| Association reassignment                         | Yes                           | No                            | Yes                       | Reassign the secondary key without deleting its previous target                                                                  |
| Entry replacement / grace                        | Yes                           | No                            | Yes                       | New mappings select the new generation; the old blob still has its original bytes during grace                                   |
| R2 multipart upload                              | Yes                           | No                            | Yes                       | Upload more than 5 MiB; download and compare the SHA-256 digest                                                                  |
| Concurrent same-key stores                       | Yes                           | No                            | Yes                       | The selected value and blob belong to one complete generation                                                                    |
| Malformed store / quota                          | Yes                           | No                            | Yes                       | `400` / `503`; preserve the previous value and mappings                                                                          |
| Maximum payload concurrency                      | Yes                           | No                            | Yes                       | Two simultaneous 64 MiB blobs with 4 MiB values succeed and preserve digests                                                     |
| Expired generation                               | Yes                           | Yes                           | Yes                       | Metadata and blob become unavailable immediately                                                                                 |
| Real Cron deletion                               | Direct scheduled-handler call | No                            | Yes                       | Scheduled cleanup removes the D1 generation and both R2 objects, and preserves accounting                                        |
| JWT claim classes / clock bounds / JWKS failures | Yes                           | Real wrong audience/event     | Real valid identity       | Full forged/fork/owner/visibility/ref/event/time matrix remains in local tests; controlled live identity exercises supplement it |
| R2 failure injection / lease and policy races    | Yes                           | Selected missing-object cases | Selected quota cases      | Local deterministic failures never publish partial state or release charges before deletion                                      |
| Operator retries and teardown guards             | Yes                           | Provisioning path             | Provisioning path         | Retry setup without duplicate resources; recover interrupted policy checks; refuse unrelated resources                           |

The main run has 14 grouped e2e checks; the PR run has eight. A group can contain several requests and assertions. The local driver test runs the same main suite, including maximum payloads, with a directly invoked scheduled handler. That does not replace the Cloudflare Cron check. Both modes also check that the advertised manual endpoint serves its expected fixture.

## Pass criteria and failure evidence

Every executed assertion must pass. The e2e command exits nonzero on failure. `report.json` records each completed group, its status, duration, deployment ID, endpoint, and authorization mode. The workflow uploads partial reports after failures too. Reports use generic error classes; credentials and opaque request bodies are excluded.

Deployment readiness permits two minutes for the expected revision and fixture to appear. Normal HTTP requests have a 30-second deadline; stores have 130 seconds. Behavior tests do not retry failed writes or turn errors into passes. Only readiness and asynchronous cleanup use bounded polling.

The main run waits at most 25 minutes for its expired fixture to disappear through the real five-minute Cron schedule. Cloudflare notes that Cron changes can take up to 15 minutes to propagate. The test also checks that both R2 objects disappear, the selected entry is gone, and charged bytes match generation totals. It never calls a fake public scheduled endpoint or performs the sentinel deletion itself.

Use the artifact and workflow logs to identify the first failed group. For server diagnosis, correlate `X-Request-Id` with sampled Worker logs. Do not retry a failed assertion solely to obtain a green result. A new run should follow a fix or an identified transient deployment issue.

## Manual verification from a PR

1. Wait for the PR comment to say that Cloudflare deployment and PR e2e checks passed. Confirm that its commit is the current PR head.
2. Open the linked workflow run. Download its `remote-cache-e2e-<run-id>-<attempt>` artifact and extract the files.
3. Read `manual-manifest.json`. It contains the expected deployment ID, endpoint, blob URL, and expected public fixture contents.
4. From the extracted directory, set `CACHE_ENDPOINT` to the manifest's endpoint and run:

```sh
curl --fail-with-body --silent --show-error \
  --dump-header fetch-headers.txt \
  --header 'Content-Type: application/cbor' \
  --data-binary @manual-fetch.cbor \
  "$CACHE_ENDPOINT/fetch" --output fetch-response.cbor
```

5. Check `X-Remote-Cache-Deployment` against the manifest. Check `Content-Type: application/cbor` and `Cache-Control: no-store`. Decode the response with a CBOR tool; expect `kind: "exact"` and the manifest's value.
6. Use `curl --fail-with-body --dump-header blob-headers.txt --output blob.txt` with the manifest's `blob_url`. Compare the downloaded text with `blob_utf8`.
7. Run `curl --silent --show-error --include --request POST "$CACHE_ENDPOINT/store"` without credentials. Expect plain-text `401`, not a login page or redirect.

For a CBOR decoder already available in this repository, run this from `packages/remote-cache` after dependency installation, replacing the file path:

```sh
node --input-type=module -e 'import { readFileSync } from "node:fs"; import { decode } from "cborg"; console.log(decode(readFileSync(process.argv[1])))' /path/to/fetch-response.cbor
```

PR previews intentionally reject publication tokens from PR workflows. Use the main staging workflow to validate successful publication. The manual namespace contains synthetic public data and expires after one day unless another run refreshes it.

## Cleanup, retries, and cost limits

Each CI deployment has a 2 GB byte budget, 1,000 entries, and 2,000 associations. Pending and retired objects remain charged. After verification, the runner expires `e2e` and `other` data and gives in-flight operations the normal ten-minute cleanup grace. It retains the `manual` fixture for developer checks. Subsequent runs explicitly restore CI-owned policy switches in case a prior process stopped during a withdrawal test.

Closing an internal PR triggers `.github/workflows/remote-cache-preview-events.yml`. The cleanup job executes code from the default branch, confirms that the PR is closed, and checks resource ownership. It disables access, waits for Cron to drain generations, then deletes the empty bucket, D1 database, and Worker. It does not delete main staging. The PR comment reports cleanup success or failure.

If Cron or orphan multipart uploads delay cleanup, the job fails instead of claiming that resources are gone. R2 lifecycle can require a day to abort an unrecorded upload. Rerun the cleanup workflow with the closed PR number after the backlog drains. Failed resource creation and partial teardown can also be retried. A nonempty bucket always blocks final removal.

Staging policies belong to CI. Do not use these names for an operator-managed production cache. Monitor resource counts and Cloudflare allowances. Resource budgets do not guarantee zero cost. Disable `REMOTE_CACHE_DEPLOY_ENABLED` to stop automatic deployments; remove existing PR resources explicitly after pending runs finish.

## Release and incident exercises

Before a production release, record the commit, workflow URL, Cloudflare plan, region, and outcome for these controlled staging exercises:

| Exercise                           | Procedure and acceptance criterion                                                                                                                                    |
| ---------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Provider failure and recovery      | Restrict D1/R2 access in staging, exercise reads and stores, then restore access. Existing mappings survive; failures remain explicit; retries recover                |
| Upload interruption                | Disconnect a real authorized multipart request after its first R2 part. Verify unchanged mappings, retained reservation, upload abortion, and eventual byte release   |
| Lease/token expiry and policy race | Delay a staging upload across expiry or change policy before publication. Neither mapping changes; old data remains readable                                          |
| Real hostile workflow identities   | Use controlled fork, tag, PR, `pull_request_target`, and `workflow_run` jobs. All writes fail; public reads still work                                                |
| Rate limits                        | Send a bounded burst to the staging namespace. Check `429`, `Retry-After`, recovery, and a finite identity set for unknown routes                                     |
| Retention and lifecycle            | Observe expiry, replacement grace, unfinished multipart abortion, and lifecycle deletion over actual retention windows                                                |
| Migration and rollback             | Upgrade a populated staging database, verify old entries, and restore a compatible previous Worker version. Never roll code back across an incompatible schema change |
| Partial restore                    | Restore D1 without deleted R2 data. Expect `503` for live missing values and `404` for missing blobs; withdraw or replace the namespace before reuse                  |
| Load and CPU                       | Measure deployed CPU, memory, D1 rows/latency, R2 operations, and cleanup lag under sustained concurrency. Compare observed costs with configured budgets             |

These are explicit release exercises, not claims that fault injection or multi-day lifecycle behavior already ran in the PR workflow. Workers Free support still needs provider CPU measurements within its limits.

References: [Cloudflare GitHub Actions deployment](https://developers.cloudflare.com/workers/ci-cd/external-cicd/github-actions/), [Cron propagation](https://developers.cloudflare.com/workers/configuration/cron-triggers/), [GitHub OIDC claims](https://docs.github.com/en/actions/reference/security/oidc), and [secure workflow use](https://docs.github.com/en/actions/reference/security/secure-use).
