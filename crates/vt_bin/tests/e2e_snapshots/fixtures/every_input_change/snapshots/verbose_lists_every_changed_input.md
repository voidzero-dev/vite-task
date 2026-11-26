# verbose_lists_every_changed_input

Without `-v`, the file check stops at the first changed file it finds, which can be any of them. With `-v`, every input is checked, and every change is listed in path order: in dry runs, in real runs, and in `--last-details` afterwards.

## `vt run read`

```
$ vtt print-file a.txt b.txt c.txt
a
b
c
```

## `vtt write-file a.txt changed`

```
```

## `vtt write-file b.txt changed`

```
```

## `vtt write-file c.txt changed`

```
```

## `vt run --dry-run read`

One changed file, whichever the check finds first.

```
$ vtt print-file a.txt b.txt c.txt ○ cache miss: '<one of a, b, c>.txt' modified
```

## `vt run --dry-run -v read`

All three, in path order.

```
$ vtt print-file a.txt b.txt c.txt ○ cache miss: 'a.txt' modified

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 0 cache hits • 1 cache miss
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] every-input-change#read: $ vtt print-file a.txt b.txt c.txt
      → Cache miss: 'a.txt' modified
                    'b.txt' modified
                    'c.txt' modified
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run -v read`

A real run checks and lists the same.

```
$ vtt print-file a.txt b.txt c.txt ○ cache miss: 'a.txt' modified, executing
changedchangedchanged

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 0 cache hits • 1 cache miss
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] every-input-change#read: $ vtt print-file a.txt b.txt c.txt ✓
      → Cache miss: 'a.txt' modified
                    'b.txt' modified
                    'c.txt' modified
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run --last-details`

The saved summary lists them too.

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 0 cache hits • 1 cache miss
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] every-input-change#read: $ vtt print-file a.txt b.txt c.txt ✓
      → Cache miss: 'a.txt' modified
                    'b.txt' modified
                    'c.txt' modified
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
