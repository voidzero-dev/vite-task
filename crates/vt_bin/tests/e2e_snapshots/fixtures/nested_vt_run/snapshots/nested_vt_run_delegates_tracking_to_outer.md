# nested_vt_run_delegates_tracking_to_outer

When a cached task spawns a nested `vt run` subprocess (wrapped via `vtt pipe-stdout` so it runs as a separate process instead of being inlined), the nested `vt` detects that automatic file tracking is already active in the outer task, runs `inner` untracked with a clear explanation, and does not pollute `inner`'s cache. Meanwhile the outer task tracks `inner`'s reads and writes (`input.txt` and `dist/out.txt`), restores `dist/out.txt` on cache hit, and invalidates when `input.txt` changes.

## `vt run -v outer`

cold run: outer caches, inner reports nested tracking

```
$ vtt pipe-stdout -- vt run -v inner
--- stdout ---
$ vtt print-file input.txt
initial

$ vtt write-file dist/out.txt built


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   2 tasks • 0 cache hits • 2 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] nested-vt-run#inner: $ vtt print-file input.txt ✓
      → Not cached: automatic file tracking is already active in a parent task. The parent task tracks these accesses; configure `input` and `output` manually to also cache this nested task.
  ·······················································
  [2] nested-vt-run#inner: $ vtt write-file dist/out.txt built ✓
      → Not cached: automatic file tracking is already active in a parent task. The parent task tracks these accesses; configure `input` and `output` manually to also cache this nested task.
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 0 cache hits • 1 cache miss
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] nested-vt-run#outer: $ vtt pipe-stdout -- vt run -v inner ✓
      → Cache miss: no previous cache entry found
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vtt rm -rf dist`

remove output produced by nested task

```
```

## `vt run -v outer`

outer cache hit restores dist/out.txt

```
$ vtt pipe-stdout -- vt run -v inner ◉ cache hit, replaying
--- stdout ---
$ vtt print-file input.txt
initial

$ vtt write-file dist/out.txt built


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   2 tasks • 0 cache hits • 2 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] nested-vt-run#inner: $ vtt print-file input.txt ✓
      → Not cached: automatic file tracking is already active in a parent task. The parent task tracks these accesses; configure `input` and `output` manually to also cache this nested task.
  ·······················································
  [2] nested-vt-run#inner: $ vtt write-file dist/out.txt built ✓
      → Not cached: automatic file tracking is already active in a parent task. The parent task tracks these accesses; configure `input` and `output` manually to also cache this nested task.
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 1 cache hit • 0 cache misses
Performance:  100% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] nested-vt-run#outer: $ vtt pipe-stdout -- vt run -v inner ✓
      → Cache hit - output replayed -
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vtt print-file dist/out.txt`

verify dist/out.txt was restored

```
built
```

## `vtt write-file input.txt updated`

modify file read only by nested grandchild

```
```

## `vt run -v outer`

outer cache miss because grandchild input changed

```
$ vtt pipe-stdout -- vt run -v inner ○ cache miss: 'input.txt' modified, executing
--- stdout ---
$ vtt print-file input.txt
updated
$ vtt write-file dist/out.txt built


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   2 tasks • 0 cache hits • 2 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] nested-vt-run#inner: $ vtt print-file input.txt ✓
      → Not cached: automatic file tracking is already active in a parent task. The parent task tracks these accesses; configure `input` and `output` manually to also cache this nested task.
  ·······················································
  [2] nested-vt-run#inner: $ vtt write-file dist/out.txt built ✓
      → Not cached: automatic file tracking is already active in a parent task. The parent task tracks these accesses; configure `input` and `output` manually to also cache this nested task.
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 0 cache hits • 1 cache miss
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] nested-vt-run#outer: $ vtt pipe-stdout -- vt run -v inner ✓
      → Cache miss: 'input.txt' modified
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run -v inner`

direct inner run misses (nested run did not pollute inner cache)

```
$ vtt print-file input.txt
updated
$ vtt write-file dist/out.txt built


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   2 tasks • 0 cache hits • 2 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] nested-vt-run#inner: $ vtt print-file input.txt ✓
      → Cache miss: no previous cache entry found
  ·······················································
  [2] nested-vt-run#inner: $ vtt write-file dist/out.txt built ✓
      → Cache miss: no previous cache entry found
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run -v inner`

second direct inner run hits cache

```
$ vtt print-file input.txt ◉ cache hit, replaying
updated
$ vtt write-file dist/out.txt built ◉ cache hit, replaying


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   2 tasks • 2 cache hits • 0 cache misses
Performance:  100% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] nested-vt-run#inner: $ vtt print-file input.txt ✓
      → Cache hit - output replayed -
  ·······················································
  [2] nested-vt-run#inner: $ vtt write-file dist/out.txt built ✓
      → Cache hit - output replayed -
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
