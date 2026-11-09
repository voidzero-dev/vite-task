# missing_archive_is_a_hit

Bug: a local hit whose output archive is gone from the cache folder is a plain hit in a dry run, which exits 0, but the real run fails to restore it. With `-vv`, the hit lists no outputs, as if it restored none, where the cached run has `dist/out.txt`.

## `vt run emit`

```
$ vtt write-file dist/out.txt built
```

## `vtt rm --ext .tar.zst node_modules/.vite/task-cache`

```
```

## `vt run --dry-run -vv emit`

Expected: the missing archive is reported. It's a hit with no `Outputs:` line.

```
$ vtt write-file dist/out.txt built ◉ cache hit

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 1 cache hit • 0 cache misses
Performance:  100% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] dry-run-and-json-bugs#emit: $ vtt write-file dist/out.txt built
      → Cache hit
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vtt pipe-stdout -- vt run --dry-run --json -vv emit`

The same in the JSON: no `outputs`.

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","plan":{"emit":{"package":".","name":"emit","steps":[{"program":"<vtt>","args":["write-file","dist/out.txt","built"],"cache":{"status":"hit","baseline":{"durationMs":<ms>}}}]}}}
```

## `vt run emit`

The real run can't restore it.

**Exit code:** 1

```
$ vtt write-file dist/out.txt built ◉ cache hit, replaying
✗ Cache restore failed. Run `vt cache clean` to clear the cache: failed to extract the output archive: <os error>
```
