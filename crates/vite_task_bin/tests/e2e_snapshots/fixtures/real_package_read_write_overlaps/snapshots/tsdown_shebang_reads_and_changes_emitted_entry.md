# tsdown_shebang_reads_and_changes_emitted_entry

After Rolldown writes a shebang entry, tsdown checks that emitted file with `access` and then changes its mode with `chmod`. The same `dist/index.js` is consequently read and mutated during one build. See [the shebang plugin](https://github.com/rolldown/tsdown/blob/49cc5f953f1b6ab13968fef8509d69767373253b/src/features/shebang.ts#L18-L34) and [`fsExists`](https://github.com/rolldown/tsdown/blob/49cc5f953f1b6ab13968fef8509d69767373253b/src/utils/fs.ts#L5-L9).

## `vt run -v overlap`

```
~/cases/tsdown-shebang$ tsdown --logLevel silent


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] tsdown-shebang-overlap#overlap: ~/cases/tsdown-shebang$ tsdown --logLevel silent ✓
      → Not cached: read and wrote 'cases/tsdown-shebang/dist/index.js'
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
