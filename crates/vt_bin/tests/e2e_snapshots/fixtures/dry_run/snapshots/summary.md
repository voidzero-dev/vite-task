# summary

## `vt run --dry-run test`

Two tasks, so there is a summary line. No blank line between commands.

```
$ vtt print-file src/a.txt
$ vtt print tested

---
vt run: 0/2 cache hit (0%), nothing was run.
```

## `vt run build`

```
$ vtt print-file src/a.txt
original
```

## `vt run --dry-run -v test`

The detailed summary has no exit icons, and the hit saves nothing.

```
$ vtt print-file src/a.txt ◉ cache hit
$ vtt print tested

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   2 tasks • 1 cache hit • 1 cache miss
Performance:  50% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] dry-run#build: $ vtt print-file src/a.txt
      → Cache hit
  ·······················································
  [2] dry-run#test: $ vtt print tested
      → Cache miss: no previous cache entry found
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run --dry-run --log labeled test`

`--log` only shapes task output, so it does nothing in a dry run.

```
$ vtt print-file src/a.txt ◉ cache hit
$ vtt print tested

---
vt run: 1/2 cache hit (50%), nothing was run.
```
