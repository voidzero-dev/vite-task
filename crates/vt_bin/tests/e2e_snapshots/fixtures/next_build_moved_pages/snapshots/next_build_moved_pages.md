# next_build_moved_pages

`next build` (Next.js 15) on a one-page site renders each static page, `index.html`, `404.html` and `500.html`, into `.next/export`, renames it into `.next/server/pages`, and removes `.next/export`.

## `vt run build`

the build log's progress lines, sizes and chunk hashes vary, so it's redacted

```
$ NEXT_TELEMETRY_DISABLED=1 next build --no-lint
 ⚠ Linting is disabled.
<next build output: build steps, page progress, route sizes and chunk hashes>
```

## `vtt rm -r .next`

remove the build output, as on a clean checkout

```
```

## `vt run build`

a cache hit restores the pages Next.js renamed into `.next/server/pages`

```
$ NEXT_TELEMETRY_DISABLED=1 next build --no-lint ◉ cache hit, replaying
 ⚠ Linting is disabled.
<next build output: build steps, page progress, route sizes and chunk hashes>


---
vt run: cache hit.
```

## `vtt stat-file .next/server/pages/index.html`

```
.next/server/pages/index.html: exists
```
