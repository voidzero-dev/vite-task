# oxfmt_reads_and_rewrites_source

In write mode, oxfmt reads each source file, formats the text, and writes changed text back to that same path. The checked-in `source.ts` is intentionally unformatted, so the file is both read and written. See [the formatter service's read and write sequence](https://github.com/oxc-project/oxc/blob/5306f24d9e82ae36ad9c3c964f33075bc589c799/apps/oxfmt/src/cli/service.rs#L42-L82) and [`read_to_string`](https://github.com/oxc-project/oxc/blob/5306f24d9e82ae36ad9c3c964f33075bc589c799/apps/oxfmt/src/core/utils.rs#L50-L53).

## `vt run -v overlap`

```
~/cases/oxfmt-in-place$ oxfmt --threads=1 source.ts
Finished in <duration> on 1 files using <n> threads.


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] oxfmt-in-place-overlap#overlap: ~/cases/oxfmt-in-place$ oxfmt --threads=1 source.ts ✓
      → Not cached: read and wrote 'cases/oxfmt-in-place/source.ts'
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
