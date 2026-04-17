# signal_terminated_task_returns_non_zero_exit_code

Tests exit code behavior for signal-terminated processes
Unix-only: Windows doesn't have Unix signals, so exit codes differ

## `vt run abort`

SIGABRT -> exit code 134

**Exit code:** 134

```
$ node -e "process.kill(process.pid, 6)"
```
