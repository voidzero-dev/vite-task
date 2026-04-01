# Cancellation

`vp run` handles two kinds of cancellation: **Ctrl-C** (user interrupt) and **fast-fail** (a task exits with non-zero status). Both prevent new tasks from being scheduled and prevent caching of in-flight results, but they differ in how they treat running processes.

## Ctrl-C

When the user presses Ctrl-C:

1. The OS delivers SIGINT (Unix) or CTRL_C_EVENT (Windows) directly to all processes in the terminal's foreground process group — both the runner and child tasks. This is standard OS behavior, not something `vp run` implements.
2. No new tasks are scheduled after the signal.
3. Results of in-flight tasks are **not cached**, even if a task handles the signal gracefully and exits 0. The output may be incomplete, so caching it would risk false cache hits on subsequent runs.

## Fast-fail

When any task exits with non-zero status:

1. All other running child processes are killed immediately (`SIGKILL` on Unix, `TerminateJobObject` on Windows).
2. No new tasks are scheduled.
3. Results of other in-flight tasks are **not cached** (they were killed mid-execution).

## Why interrupted tasks are not cached

A task that receives Ctrl-C might exit 0 after partial work (e.g., a build tool that flushes what it has so far). Caching this result would mean the next `vp run` replays incomplete output and skips the real execution. By never caching interrupted results, `vp run` guarantees that the next run starts fresh.
