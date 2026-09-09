# Vite SaaS frontend artifact measurements

Measured on 2026-09-07 for [the remote-cache RFC](../0001-remote-cache.md#13-free-and-paid-capacity-comparison).

The four sampled frontend outputs compress to **4.77–34.71 MB** with zstd level 3. A 5 MB result is a useful small-output case, but three of these four releases exceed it. Use a 50 MB planning case for mature SaaS frontends alongside the smaller scenarios. This sample does not establish the average cache size across users or tasks.

## Projects and scope

We selected established products with public source and downloadable release artifacts. Each repository had more than 20,000 GitHub stars when checked; this selects recognizable projects, not a random sample of Vite users. The sample includes commercial products with different source licenses.

| Project                                                | Product                      | GitHub stars at observation | Measured release                       | Published frontend output                          |
| ------------------------------------------------------ | ---------------------------- | --------------------------: | -------------------------------------- | -------------------------------------------------- |
| [Directus](https://github.com/directus/directus)       | Data platform / headless CMS |                      37,786 | `@directus/app@17.1.1`, from `v12.3.1` | npm package `dist/`                                |
| [Docmost](https://github.com/docmost/docmost)          | Collaborative wiki           |                      21,602 | `v0.95.0`                              | Docker build's `/app/apps/client/dist/` COPY layer |
| [Hoppscotch](https://github.com/hoppscotch/hoppscotch) | API development platform     |                      80,228 | `2026.8.0`                             | Docker build's `/site/selfhost-web/` COPY layer    |
| [n8n](https://github.com/n8n-io/n8n)                   | Workflow automation          |                     203,576 | `n8n-editor-ui@2.16.2`                 | npm package `dist/`                                |

These are measurements of **published build products**. We did not run local builds or measure the vendors' private cloud deployments. Docker samples use `linux/amd64` manifests and the layer that copies the frontend from the build stage, before runtime dependency installation or startup transformations. We exclude the Docker base image, server code, dependencies, and unrelated npm package files.

The pinned source confirms Vite usage:

| Project    | Build evidence                                                                                                                                                                       | Output/configuration evidence                                                                                                                                                                                                                                                                 |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Directus   | [`build: vite build`](https://github.com/directus/directus/blob/973be10df8b0305569dc0dc53e187c648133c8d6/app/package.json)                                                           | [Vite configuration](https://github.com/directus/directus/blob/973be10df8b0305569dc0dc53e187c648133c8d6/app/vite.config.js)                                                                                                                                                                   |
| Docmost    | [`build: tsc && vite build`](https://github.com/docmost/docmost/blob/4132dd597c956a27423607d008708c0e214690da/apps/client/package.json)                                              | [Vite configuration](https://github.com/docmost/docmost/blob/4132dd597c956a27423607d008708c0e214690da/apps/client/vite.config.ts), [Dockerfile COPY](https://github.com/docmost/docmost/blob/4132dd597c956a27423607d008708c0e214690da/Dockerfile)                                             |
| Hoppscotch | [`generate` calls `build`, which invokes Vite](https://github.com/hoppscotch/hoppscotch/blob/ac145e7f758151b41fd46d3e5f513886ce9068ba/packages/hoppscotch-selfhost-web/package.json) | [Vite configuration](https://github.com/hoppscotch/hoppscotch/blob/ac145e7f758151b41fd46d3e5f513886ce9068ba/packages/hoppscotch-selfhost-web/vite.config.ts), [production Dockerfile](https://github.com/hoppscotch/hoppscotch/blob/ac145e7f758151b41fd46d3e5f513886ce9068ba/prod.Dockerfile) |
| n8n        | [`build` invokes `vite build`](https://github.com/n8n-io/n8n/blob/9bdde69954a4d7d1569d37b6fd3a3f55f55b297a/packages/frontend/editor-ui/package.json)                                 | [Vite configuration](https://github.com/n8n-io/n8n/blob/9bdde69954a4d7d1569d37b6fd3a3f55f55b297a/packages/frontend/editor-ui/vite.config.mts)                                                                                                                                                 |

[sources.json](sources.json) records artifact URLs, SHA-256 digests, source commits, npm provenance URLs, and Docker manifest/layer digests. We checked npm downloads against their registry SHA-512 integrity values and Docker layers against their SHA-256 digests. We obtained npm source commits from the registry's provenance records; the Docker source references identify the corresponding release tags. We did not verify attestation signatures or independently rebuild the releases.

## Measured sizes

All sizes use decimal MB (`1 MB = 1,000,000 bytes`). “Output” sums regular-file bytes, including static assets and source maps present in the selected directory. “Archive” measures a single sorted GNU tar stream compressed with `zstd -3 -T1`, using zstd `1.5.7`. We normalize tar paths and metadata. Archive sizes estimate the compressed output payload; a complete cache entry also includes task metadata.

| Project    | Files | Output MB | Archive MB | Source-map MB before compression | Archive MB with `.map` files omitted |
| ---------- | ----: | --------: | ---------: | -------------------------------: | -----------------------------------: |
| Directus   |   401 |     20.81 |   **6.97** |                                0 |                                 6.97 |
| Docmost    |   230 |     14.83 |   **4.77** |                                0 |                                 4.77 |
| Hoppscotch |   471 |    127.53 |  **32.18** |                            89.82 |                                12.18 |
| n8n        | 1,477 |    162.76 |  **34.71** |                           118.66 |                                13.15 |

Source maps comprise about 70% of Hoppscotch's uncompressed output and 73% of n8n's. Their configurations enable source maps for these builds; n8n also enables its legacy-browser plugin for releases. Hoppscotch includes a TypeScript worker, while n8n includes worker and WebAssembly assets. A page's initial JavaScript download omits much of this build output and cannot substitute for the cache size.

The last column is a controlled sensitivity calculation on the same files, not another build. Removing maps reduces the compressed archives to 12.18 MB and 13.15 MB respectively, still above 5 MB. A cache must preserve the outputs required by its task; these measurements do not justify silently dropping source maps. For Directus and Docmost, the published frontend directories contain no `.map` files.

These archives contain frontend output files only. A complete cached task also needs terminal events and client validation metadata. Under [PR #713](https://github.com/voidzero-dev/vite-task/pull/713), the client encodes these in an opaque value and optional blob; the Worker does not prescribe their internal format. A real cached task can produce additional files outside `dist/`; release packaging can omit such files. Measure those bytes during implementation before assigning a complete per-result size. Backend and shared-package builds are separate task results unless the operator caches them as one task.

## Effect on the free storage budget

Using the RFC's 8 GB application budget and seven-day retention, the measured output archives alone give these steady-state ceilings when each store creates a different exact key:

| Project-sized result | New results/day within 8 GB | Storage at 200 new results/day, seven days |
| -------------------- | --------------------------: | -----------------------------------------: |
| Directus             |                         164 |                                    9.75 GB |
| Docmost              |                         239 |                                    6.68 GB |
| Hoppscotch           |                          35 |                                   45.06 GB |
| n8n                  |                          32 |                                   48.59 GB |

Calculate the ceiling as `floor(8,000,000,000 / (archive_bytes * 7))`. These are artifact-only upper bounds; client metadata, logs, pending uploads, retired generations, and delayed deletion lower them. A new exact key stores another complete archive even when many assets match the previous build. Repeated stores to the same key replace its value and blob, keeping the old generation only for a short download grace period. Input changes do not necessarily create a different exact key. Read hits do not create another copy. Count separate namespaces and client compatibility identities where the service stores separate results.

For a **50 MB complete-result planning case**, the same budget supports at most **22 new distinct keys/day** with seven-day retention, before operational headroom. At 200 new distinct keys/day, storage reaches 70 GB. Under the RFC's operation assumptions, raising the storage budget would cost about **$0.90/month in R2 storage** beyond the 10 GB included allowance. Workers could remain Free if the streaming implementation meets its CPU limit. This uses the RFC's steady-state 30-day billing model and [R2 Standard pricing](https://developers.cloudflare.com/r2/pricing/), rechecked on 2026-09-09. The default profile would reject additional stores instead of raising its budget.

Keep 5 MB for a small-output scenario and 50 MB for mature frontend builds. All four measured archives fit the RFC's 64 MiB blob limit. These values are planning inputs, not estimates of population averages. A few new full-frontend keys per day can fit the free budget; hundreds of different keys per day need smaller results, shorter retention, or additional storage. Frequent overwrites can retain much less data but still consume operations. The number of readers does not determine this storage case. In v1, only explicit main-branch CI publication adds remote results; public readers need no credentials or per-user subscription.

This sample covers one release per product and favors mature applications. It does not measure a store-weighted average, daily change rate, exact-key replacement rate, cache hit rate, or the fraction of ordinary users that stay free. The RFC's canary still needs those measurements across task types and successive changes.

## Reproduce

Run from the repository root with Python 3 and the `zstd` CLI installed:

```sh
python3 docs/rfcs/remote-cache-size-study/measure.py \
  --cache-dir /tmp/vite-cache-size-study \
  --output /tmp/vite-cache-size-study-results.json
```

The script downloads about 84 MB of pinned npm archives and Docker layers, checks SHA-256 digests, and decompresses them into temporary tar files for reading. It does not install packages, execute project code, or start containers. It writes compressed comparison archives in the cache directory. Allow about 600 MB of disk space. Registry availability and anonymous Docker pull limits can affect reruns.

[results.json](results.json) contains exact byte counts, compressed archive digests, file-type totals, and the five largest files for each sample. Use those counts for calculations; the tables round values for readability.
