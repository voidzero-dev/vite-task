# command_scope

Reports apply to one command, including a standalone marker and actual nested runner processes.

## `vt run --verbose compound`

```
$ vtt report-unchanged
command ran
◉ unchanged (reported by task)

$ vtt print changed
changed


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   2 tasks • 0 cache hits • 2 cache misses • 1 unchanged
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] reported-unchanged#compound: $ vtt report-unchanged ✓
      → Unchanged (reported by task)
      → Cache miss: no previous cache entry found
  ·······················································
  [2] reported-unchanged#compound: $ vtt print changed ✓
      → Cache miss: no previous cache entry found
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run --verbose marker`

```
$ vtt print changed ◉ cache hit, replaying
changed

$ vt run --report-unchanged ⊘ cache disabled
◉ unchanged (reported by task)


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   2 tasks • 1 cache hits • 0 cache misses • 1 cache disabled • 1 unchanged
Performance:  50% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] reported-unchanged#marker: $ vtt print changed ✓
      → Cache hit - output replayed -
  ·······················································
  [2] reported-unchanged#marker: $ vt run --report-unchanged ✓
      → Unchanged (reported by task)
      → Cache disabled in task configuration
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run --verbose nested`

```
$ vtt report-unchanged nested ⊘ cache disabled
$ vtt report-unchanged ⊘ cache disabled
command ran
◉ unchanged (reported by task)

---
vt run: unchanged (reported by task).


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 0 cache misses • 1 cache disabled
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] reported-unchanged#nested: $ vtt report-unchanged nested ✓
      → Cache disabled in task configuration
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
