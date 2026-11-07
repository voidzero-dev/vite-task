# env_and_file_changed

When an env in `cache.env` changes, the miss also compares the task's files with its previous cached run, so it names what reverting needs. Dry runs record nothing, so each step compares with the same cached run.

## `MY_ENV=1 vt run test`

```
$ vtt print-file test.txt
initial content
```

## `vtt replace-file-content test.txt initial changed`

```
```

## `MY_ENV=2 vt run --dry-run -v test`

The env and the file both changed.

```
$ vtt print-file test.txt ○ cache miss: env 'MY_ENV' changed

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 0 cache hits • 1 cache miss
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] cache-miss-reasons#test: $ vtt print-file test.txt
      → Cache miss: env 'MY_ENV' changed
                    'test.txt' modified
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `MY_ENV=1 vt run --dry-run test`

Reverting the env isn't enough.

```
$ vtt print-file test.txt ○ cache miss: 'test.txt' modified
```

## `vtt replace-file-content test.txt changed initial`

```
```

## `MY_ENV=1 vt run --dry-run test`

Reverting the file too is a hit.

```
$ vtt print-file test.txt ◉ cache hit

---
vt run: cache hit, nothing was run.
```
