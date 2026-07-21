# tsdown_clean_reads_and_rewrites_dist

tsdown expands the configured clean paths with `tinyglobby` and removes every matched entry before the bundle writes its replacement. With an existing `dist/index.js`, the build therefore observes and removes that path before emitting `dist/index.js` again. See [`cleanOutDir`](https://github.com/rolldown/tsdown/blob/49cc5f953f1b6ab13968fef8509d69767373253b/src/features/clean.ts#L14-L45).

## `vt run -v overlap`

```
~/cases/tsdown-clean$ tsdown --logLevel silent


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] tsdown-clean-overlap#overlap: ~/cases/tsdown-clean$ tsdown --logLevel silent ✓
      → Not cached: read and wrote 'cases/tsdown-clean/dist/index.js'
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
