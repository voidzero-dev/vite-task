# predicts_hits_and_misses

`--dry-run` reports each task's local cache result without running it. A task whose dependency isn't a hit is unknown. The dry run writes nothing, so it creates no cache and doesn't replace the last summary.

## `vt run --dry-run test`

no cache yet: build misses, test is unknown

```
dry-run#build: $ vtt print-file src.txt → Cache miss: no previous cache entry found
dry-run#test: $ vtt print-file test.txt → Unknown: runs after 'dry-run#build', which isn't a cache hit
```

## `vtt stat-file node_modules`

the dry run created no cache

```
node_modules: missing
```

## `vt run test`

populate the cache

```
$ vtt print-file src.txt
source

$ vtt print-file test.txt
tests

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

## `vt run --dry-run test`

both hit

```
dry-run#build: $ vtt print-file src.txt → Cache hit
dry-run#test: $ vtt print-file test.txt → Cache hit
```

## `vtt replace-file-content src.txt source changed`

modify build's input

```
```

## `vt run --dry-run test`

build misses with the reason, test is unknown

```
dry-run#build: $ vtt print-file src.txt → Cache miss: 'src.txt' modified
dry-run#test: $ vtt print-file test.txt → Unknown: runs after 'dry-run#build', which isn't a cache hit
```

## `vt run --dry-run test`

unchanged: the dry run didn't update the cache

```
dry-run#build: $ vtt print-file src.txt → Cache miss: 'src.txt' modified
dry-run#test: $ vtt print-file test.txt → Unknown: runs after 'dry-run#build', which isn't a cache hit
```

## `vt run --last-details`

still the summary of the last real run

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   2 tasks • 0 cache hits • 2 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] dry-run#build: $ vtt print-file src.txt ✓
      → Cache miss: no previous cache entry found
  ·······················································
  [2] dry-run#test: $ vtt print-file test.txt ✓
      → Cache miss: no previous cache entry found
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run test`

the real run reports the same miss

```
$ vtt print-file src.txt ○ cache miss: 'src.txt' modified, executing
changed

$ vtt print-file test.txt ◉ cache hit, replaying
tests

---
vt run: 1/2 cache hit (50%). (Run `vt run --last-details` for full details)
```

## `vt run --dry-run test`

both hit again

```
dry-run#build: $ vtt print-file src.txt → Cache hit
dry-run#test: $ vtt print-file test.txt → Cache hit
```
