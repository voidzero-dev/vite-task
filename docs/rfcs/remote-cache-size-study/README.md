# Vite SaaS frontend artifact measurements

Measured on 2026-09-07 for [the remote-cache RFC](../0001-remote-cache.md#13-free-and-paid-capacity-comparison).

The four sampled frontend outputs compress to **4.77–34.71 MB** with zstd level 3. A 5 MB result is useful for a scenario with small outputs. Three of these four releases exceed it. Use an additional 50 MB planning case for mature SaaS frontends. This sample does not establish the average cache size across users or tasks.

## Projects and scope

We selected established products with public source and downloadable release artifacts. Each repository had more than 20,000 GitHub stars when we checked it. This method selects recognizable projects. It does not produce a random sample of Vite users. The sample includes commercial products with different source licenses.

| Project                                                | Product                      | GitHub stars at observation | Measured release                       | Published frontend output                          |
| ------------------------------------------------------ | ---------------------------- | --------------------------: | -------------------------------------- | -------------------------------------------------- |
| [Directus](https://github.com/directus/directus)       | Data platform / headless CMS |                      37,786 | `@directus/app@17.1.1`, from `v12.3.1` | npm package `dist/`                                |
| [Docmost](https://github.com/docmost/docmost)          | Collaborative wiki           |                      21,602 | `v0.95.0`                              | Docker build's `/app/apps/client/dist/` COPY layer |
| [Hoppscotch](https://github.com/hoppscotch/hoppscotch) | API development platform     |                      80,228 | `2026.8.0`                             | Docker build's `/site/selfhost-web/` COPY layer    |
| [n8n](https://github.com/n8n-io/n8n)                   | Workflow automation          |                     203,576 | `n8n-editor-ui@2.16.2`                 | npm package `dist/`                                |

These measurements cover **published build products**. We did not run local builds or measure the vendors' private cloud deployments. Docker samples use `linux/amd64` manifests. We selected the layer that copies the frontend from the build stage. This layer precedes runtime dependency installation and startup transformations.

We exclude the Docker base image, server code, dependencies, and unrelated npm package files.

The pinned source confirms Vite usage:

| Project    | Build evidence                                                                                                                                                                       | Output/configuration evidence                                                                                                                                                                                                                                                                 |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Directus   | [`build: vite build`](https://github.com/directus/directus/blob/973be10df8b0305569dc0dc53e187c648133c8d6/app/package.json)                                                           | [Vite configuration](https://github.com/directus/directus/blob/973be10df8b0305569dc0dc53e187c648133c8d6/app/vite.config.js)                                                                                                                                                                   |
| Docmost    | [`build: tsc && vite build`](https://github.com/docmost/docmost/blob/4132dd597c956a27423607d008708c0e214690da/apps/client/package.json)                                              | [Vite configuration](https://github.com/docmost/docmost/blob/4132dd597c956a27423607d008708c0e214690da/apps/client/vite.config.ts), [Dockerfile COPY](https://github.com/docmost/docmost/blob/4132dd597c956a27423607d008708c0e214690da/Dockerfile)                                             |
| Hoppscotch | [`generate` calls `build`, which invokes Vite](https://github.com/hoppscotch/hoppscotch/blob/ac145e7f758151b41fd46d3e5f513886ce9068ba/packages/hoppscotch-selfhost-web/package.json) | [Vite configuration](https://github.com/hoppscotch/hoppscotch/blob/ac145e7f758151b41fd46d3e5f513886ce9068ba/packages/hoppscotch-selfhost-web/vite.config.ts), [production Dockerfile](https://github.com/hoppscotch/hoppscotch/blob/ac145e7f758151b41fd46d3e5f513886ce9068ba/prod.Dockerfile) |
| n8n        | [`build` invokes `vite build`](https://github.com/n8n-io/n8n/blob/9bdde69954a4d7d1569d37b6fd3a3f55f55b297a/packages/frontend/editor-ui/package.json)                                 | [Vite configuration](https://github.com/n8n-io/n8n/blob/9bdde69954a4d7d1569d37b6fd3a3f55f55b297a/packages/frontend/editor-ui/vite.config.mts)                                                                                                                                                 |

[sources.json](sources.json) records artifact URLs, SHA-256 digests, source commits, npm provenance URLs, and Docker manifest/layer digests. We checked npm downloads against the registry's SHA-512 integrity values. We checked Docker layers against their SHA-256 digests.

We obtained npm source commits from the registry's provenance records. The Docker source references identify the corresponding release tags. We did not verify attestation signatures or independently rebuild the releases.

## Measured sizes

All sizes use decimal MB (`1 MB = 1,000,000 bytes`). “Output” sums regular-file bytes, including static assets and source maps in the selected directory. “Archive” measures one sorted GNU tar stream compressed with `zstd -3 -T1`, using zstd `1.5.7`. We normalize tar paths and metadata. Archive sizes estimate the compressed output payload. A complete cache entry also includes task metadata.

| Project    | Files | Output MB | Archive MB | Source-map MB before compression | Archive MB with `.map` files omitted |
| ---------- | ----: | --------: | ---------: | -------------------------------: | -----------------------------------: |
| Directus   |   401 |     20.81 |   **6.97** |                                0 |                                 6.97 |
| Docmost    |   230 |     14.83 |   **4.77** |                                0 |                                 4.77 |
| Hoppscotch |   471 |    127.53 |  **32.18** |                            89.82 |                                12.18 |
| n8n        | 1,477 |    162.76 |  **34.71** |                           118.66 |                                13.15 |

Source maps account for about 70% of Hoppscotch's uncompressed output and 73% of n8n's. Their configurations enable source maps for these builds. n8n also enables its legacy-browser plugin for releases. Hoppscotch includes a TypeScript worker. n8n includes worker and WebAssembly assets.

A page's initial JavaScript download omits much of this build output. Its size cannot substitute for the cache size.

The last column measures the same files without source maps. It does not represent another build. Without maps, Hoppscotch's compressed archive measures 12.18 MB and n8n's measures 13.15 MB. Both remain above 5 MB.

A cache must preserve the outputs that its task requires. These measurements do not justify silent removal of source maps. The published frontend directories for Directus and Docmost contain no `.map` files.

These archives contain frontend output files only. A complete cached task also needs terminal events and client validation metadata. Under [PR #713](https://github.com/voidzero-dev/vite-task/pull/713), the client encodes these in an opaque value and optional blob. The Worker does not define or interpret their internal format.

A cached task can produce additional files outside `dist/`. Release packaging can omit these files. Measure their bytes during implementation before you assign a complete size to each result. Backend and shared-package builds are separate task results unless the operator caches them as one task.

## Effect on the free storage budget

The table uses the RFC's 8 GB application budget and seven-day retention. Each store creates a different exact key at a steady rate. The ceilings include only the measured output archives:

| Project-sized result | New results/day within 8 GB | Storage at 200 new results/day, seven days |
| -------------------- | --------------------------: | -----------------------------------------: |
| Directus             |                         164 |                                    9.75 GB |
| Docmost              |                         239 |                                    6.68 GB |
| Hoppscotch           |                          35 |                                   45.06 GB |
| n8n                  |                          32 |                                   48.59 GB |

Calculate the ceiling as `floor(8,000,000,000 / (archive_bytes * 7))`. These upper bounds include only artifacts. Client metadata, logs, pending uploads, retired generations, and delayed deletion reduce them.

A new exact key stores another complete archive, even when many assets match the previous build. Repeated stores to the same key replace its value and blob. The old generation remains only for a short download grace period. Input changes do not necessarily create a different exact key. Read hits do not create another copy. Count separate namespaces and client compatibility identities when the service stores separate results.

For a **50 MB planning case for complete results**, the same budget supports at most **22 new distinct keys/day** with seven-day retention. This excludes spare capacity for operation. At 200 new distinct keys/day, storage reaches 70 GB.

Under the RFC's operation assumptions, this higher budget costs about **$0.90/month in R2 storage** beyond the included 10 GB. Workers can remain Free if the streaming implementation meets its CPU limit. The estimate uses steady usage, a 30-day month, and [R2 Standard pricing](https://developers.cloudflare.com/r2/pricing/), which we rechecked on 2026-09-09. The default profile rejects additional stores instead of increasing its budget.

Use 5 MB for a scenario with small outputs and 50 MB for mature frontend builds. All four measured archives fit the RFC's 64 MiB blob limit. These values are planning inputs. They do not estimate population averages.

A few new keys for complete frontend outputs each day can fit the free budget. Hundreds of different keys each day require smaller results, shorter retention, or more storage. Frequent overwrites can retain much less data but still consume operations.

The number of readers does not determine this storage case. In version 1, only explicit CI publication from the main branch adds remote results. Public readers need no credentials or subscription for each user.

This sample covers one release per product and favors mature applications. It does not measure these values:

- Average size weighted by store counts.
- Daily change rate.
- Exact-key replacement rate.
- Cache hit rate.
- The fraction of ordinary users that stay free.

The RFC's limited production trial still needs these measurements across task types and successive changes.

## Reproduce

Run from the repository root with Python 3 and the `zstd` CLI installed:

```sh
python3 docs/rfcs/remote-cache-size-study/measure.py \
  --cache-dir /tmp/vite-cache-size-study \
  --output /tmp/vite-cache-size-study-results.json
```

The script downloads about 84 MB of pinned npm archives and Docker layers. It checks SHA-256 digests and decompresses the downloads into temporary tar files for inspection. It does not install packages, execute project code, or start containers. It writes compressed comparison archives in the cache directory.

Allow about 600 MB of disk space. Registry availability and anonymous Docker pull limits can affect repeated runs.

[results.json](results.json) contains exact byte counts, compressed archive digests, file-type totals, and the five largest files for each sample. Use those counts for calculations. The tables round values for readability.
