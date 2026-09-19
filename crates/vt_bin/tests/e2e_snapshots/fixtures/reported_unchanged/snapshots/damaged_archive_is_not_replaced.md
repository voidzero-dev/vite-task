# damaged_archive_is_not_replaced

An unreadable existing output archive prevents caching the new inputs instead of silently discarding the outputs.

## `vt run build`

```
$ vtt report-unchanged build input.txt artifact.txt
built artifact
```

## `vtt report-unchanged corrupt-archive`

```
```

## `vtt write-file input.txt unchanged`

```
```

## `vt run --verbose build`

**Exit code:** 1

```
$ vtt report-unchanged build input.txt artifact.txt ○ cache miss: 'input.txt' modified, executing
command ran
✗ Cache update failed: Unknown frame descriptor


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses • 1 failed
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] reported-unchanged#build: $ vtt report-unchanged build input.txt artifact.txt ✓
      → Cache miss: 'input.txt' modified
      ✗ Error: Cache update failed: Unknown frame descriptor
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run --verbose build`

**Exit code:** 1

```
$ vtt report-unchanged build input.txt artifact.txt ○ cache miss: 'input.txt' modified, executing
command ran
✗ Cache update failed: Unknown frame descriptor


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses • 1 failed
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] reported-unchanged#build: $ vtt report-unchanged build input.txt artifact.txt ✓
      → Cache miss: 'input.txt' modified
      ✗ Error: Cache update failed: Unknown frame descriptor
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
