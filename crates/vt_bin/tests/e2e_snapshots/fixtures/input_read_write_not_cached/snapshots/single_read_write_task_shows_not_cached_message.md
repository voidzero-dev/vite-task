# single_read_write_task_shows_not_cached_message

A single task that reads and writes the same file (fspy sees both ops) should be flagged as "not cached because it modified its inputs" in the compact summary. `--last-details` then names the file and shows the `cache` settings that exclude it, relative to the package.

## `vt run task`

```
~/packages/rw-pkg$ vtt replace-file-content src/data.txt i !

---
vt run: @test/rw-pkg#task not cached because it modified its inputs. (Run `vt run --last-details` for full details)
```

## `vt run task`

```
~/packages/rw-pkg$ vtt replace-file-content src/data.txt i !

---
vt run: @test/rw-pkg#task not cached because it modified its inputs. (Run `vt run --last-details` for full details)
```

## `vt run --last-details`

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 0 cache hits • 1 cache miss
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] @test/rw-pkg#task: ~/packages/rw-pkg$ vtt replace-file-content src/data.txt i ! ✓
      → Not cached: the task read and wrote 'packages/rw-pkg/src/data.txt'
        If this file is temporary or shouldn't affect caching, exclude it (or a glob matching it) in the task's `cache` config:
          input: [{ auto: true }, "!src/data.txt"],
          output: [{ auto: true }, "!src/data.txt"],
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
