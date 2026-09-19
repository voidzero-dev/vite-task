# interruption_invalidates_report

An acknowledged report is discarded after Ctrl+C, including a child that handles the signal and exits successfully.

## `vt run --verbose interrupted`

**→ expect-milestone:** `ready`

```
$ vtt report-unchanged interrupt
command ran
```

**← write-key:** `ctrl-c`

```
$ vtt report-unchanged interrupt
command ran
ctrl-c received


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] reported-unchanged#interrupted: $ vtt report-unchanged interrupt ✓
      → Cache miss: no previous cache entry found
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run --verbose interrupted`

**→ expect-milestone:** `ready`

```
$ vtt report-unchanged interrupt
command ran
```

**← write-key:** `ctrl-c`

```
$ vtt report-unchanged interrupt
command ran
ctrl-c received


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] reported-unchanged#interrupted: $ vtt report-unchanged interrupt ✓
      → Cache miss: no previous cache entry found
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
