# leftover_process_holding_stdout

The task exits but leaves a process that holds its stdout and stderr for 30 s. The runner reads them for a grace period after the task exits and then stops waiting, so the run ends long before the step timeout. The run isn't cached, since the captured output may be incomplete. On Windows the task's job object ends such processes when the task exits.

## `vt run leave-stdout`

ends after the grace period, not after 30 s

```
$ vtt linger 30000 stdio
left a process running (stdio)
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
  [1] leftover-process#leave-stdout: $ vtt linger 30000 stdio ✓
      → Not cached: the task exited, but a process it started kept its output or runner connection open
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run leave-stdout`

runs again: the first run wasn't cached

```
$ vtt linger 30000 stdio
left a process running (stdio)
```
