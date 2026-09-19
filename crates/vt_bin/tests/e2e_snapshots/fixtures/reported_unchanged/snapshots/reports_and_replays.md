# reports_and_replays

Reports are distinct from cache hits; explicit, automatic, uncached, and CLI reporting work.

## `vt run report`

```
$ vtt report-unchanged
command ran
◉ unchanged (reported by task)

---
vt run: unchanged (reported by task).
```

## `vt run report`

```
$ vtt report-unchanged ◉ cache hit, replaying
command ran

---
vt run: cache hit.
```

## `vt run auto`

```
$ vtt report-unchanged
command ran
◉ unchanged (reported by task)

---
vt run: unchanged (reported by task).
```

## `vt run auto`

```
$ vtt report-unchanged ◉ cache hit, replaying
command ran

---
vt run: cache hit.
```

## `vt run uncached`

```
$ vtt report-unchanged ⊘ cache disabled
command ran
◉ unchanged (reported by task)

---
vt run: unchanged (reported by task).
```

## `vt run uncached`

```
$ vtt report-unchanged ⊘ cache disabled
command ran
◉ unchanged (reported by task)

---
vt run: unchanged (reported by task).
```

## `vt run cli`

```
$ vtt report-unchanged cli
command ran
◉ unchanged (reported by task)

---
vt run: unchanged (reported by task).
```

## `vt run --last-details`

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses • 1 unchanged
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] reported-unchanged#cli: $ vtt report-unchanged cli ✓
      → Unchanged (reported by task)
      → Cache miss: no previous cache entry found
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
