# leftover_process_holding_runner_connection

The task exits but leaves a process connected to the runner's IPC server for 30 s. The runner lets the connection drain for a grace period after the task exits and then stops waiting, so the run ends long before the step timeout. The run isn't cached, since that process could still report inputs or disable caching. On Windows the task's job object ends such processes when the task exits.

## `vt run leave-connection`

ends after the grace period, not after 30 s

```
$ vtt linger 30000 ipc
left a process running (ipc)
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
  [1] leftover-process#leave-connection: $ vtt linger 30000 ipc ✓
      → Not cached: the task exited, but a process it started kept its output or runner connection open
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run leave-connection`

runs again: the first run wasn't cached

```
$ vtt linger 30000 ipc
left a process running (ipc)
```
