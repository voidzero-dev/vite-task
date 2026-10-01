# read_write_outside_package_suggests_workspace_base

When the overlapping file is outside the task's package, the suggested exclusion uses the workspace as its base, because a plain pattern is relative to the package.

## `vt run task-outside`

```
~/packages/rw-pkg$ vtt replace-file-content ../../shared.txt i !

---
vt run: @test/rw-pkg#task-outside not cached because it modified its inputs. (Run `vt run --last-details` for full details)
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
  [1] @test/rw-pkg#task-outside: ~/packages/rw-pkg$ vtt replace-file-content ../../shared.txt i ! ✓
      → Not cached: the task read and wrote 'shared.txt'
        If this file is temporary or shouldn't affect caching, exclude it (or a glob matching it) in the task's `cache` config:
          input: [{ auto: true }, { pattern: "!shared.txt", base: "workspace" }],
          output: [{ auto: true }, { pattern: "!shared.txt", base: "workspace" }],
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
