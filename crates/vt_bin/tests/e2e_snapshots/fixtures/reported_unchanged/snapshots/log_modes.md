# log_modes

Every output mode and saved details display unchanged without adding cache hits or saved time.

## `vt run --no-cache --log interleaved report`

```
$ vtt report-unchanged ⊘ cache disabled
command ran
◉ unchanged (reported by task)

---
vt run: unchanged (reported by task).
```

## `vt run --no-cache --log labeled report`

```
[reported-unchanged#report] $ vtt report-unchanged ⊘ cache disabled
[reported-unchanged#report] command ran
◉ unchanged (reported by task)

---
vt run: unchanged (reported by task).
```

## `vt run --no-cache --log grouped report`

```
[reported-unchanged#report] $ vtt report-unchanged ⊘ cache disabled
── [reported-unchanged#report] ──
command ran
◉ unchanged (reported by task)

---
vt run: unchanged (reported by task).
```

## `vt run --no-cache --verbose report`

```
$ vtt report-unchanged ⊘ cache disabled
command ran
◉ unchanged (reported by task)


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 0 cache misses • 1 cache disabled • 1 unchanged
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] reported-unchanged#report: $ vtt report-unchanged ✓
      → Unchanged (reported by task)
      → Cache disabled in task configuration
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run --last-details`

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 0 cache misses • 1 cache disabled • 1 unchanged
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] reported-unchanged#report: $ vtt report-unchanged ✓
      → Unchanged (reported by task)
      → Cache disabled in task configuration
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
