# report_unchanged_from_js

## `node scripts/report_unchanged.mjs`

```
JS command ran
```

## `vt run report-unchanged`

```
$ node scripts/report_unchanged.mjs
JS command ran
◉ unchanged (reported by task)

---
vt run: unchanged (reported by task).
```

## `vt run report-unchanged`

```
$ node scripts/report_unchanged.mjs ◉ cache hit, replaying
JS command ran

---
vt run: cache hit.
```

## `vt run --no-cache report-unchanged`

```
$ node scripts/report_unchanged.mjs ⊘ cache disabled
JS command ran
◉ unchanged (reported by task)

---
vt run: unchanged (reported by task).
```

## `vt run --verbose report-unchanged --fail`

**Exit code:** 7

```
$ node scripts/report_unchanged.mjs --fail
JS command ran


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses • 1 failed
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] ipc-client-test#report-unchanged: $ node scripts/report_unchanged.mjs --fail ✗ (exit code: 7)
      → Cache miss: no previous cache entry found
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
