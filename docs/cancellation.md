# Cancellation

`vp run` handles two kinds of cancellation: **Ctrl-C** (user interrupt) and **fast-fail** (a task exits with non-zero status). Both prevent new tasks from being scheduled, prevent caching of in-flight results, and stop [remote cache lookups](#remote-cache-lookups), but they differ in how they treat running processes and [remote cache uploads](#remote-cache-uploads).

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

With `--continue` (or `--no-bail`, as in pnpm), a failed task doesn't cancel the run. Running tasks aren't killed, new tasks keep being scheduled, and tasks that succeed are cached as usual. Tasks that depend on a failed task, directly or through other tasks, don't run. Like tasks that fast-fail never started, they aren't shown in the summary. A task whose script runs `vp run` fails if any task in that run fails, and in a `&&`-chained command, the commands after a failed one don't run. Once every task that can run has finished, `vp run` exits with a non-zero status: the failed task's exit code if one task failed, or 1 if more did. Ctrl-C still cancels the run.

`--continue` carries over to every `vp run` in the scripts of the tasks it runs, including one that a task starts as a separate process. A `vp run --continue` in a task's script applies to the tasks that it runs: when one of them fails, the others keep going, and once they're done, the task with that script fails like any other. If the outer `vp run` has no `--continue`, it then fast-fails, and a failure elsewhere in the outer run kills the tasks still running in the nested one.

## Remote cache lookups

Both kinds of cancellation stop remote cache lookups and downloads right away instead of waiting for them to finish or time out. A task whose cache lookup was still in progress doesn't start and doesn't restore cached outputs, even if the lookup found them. Like a task that was never scheduled, it isn't shown in the summary.

## Remote cache uploads

In `read-write` mode, a task's upload to the remote cache starts once its result is cached locally and keeps running after the task finishes, so tasks that depend on it don't wait for it. Once all tasks are done, `vp run` waits for the uploads still running before it prints the summary, with a message saying how many there are.

- Ctrl-C cancels them. Pressed while `vp run` waits, it cancels the uploads right away. Pressed while tasks are running, it cancels the uploads still running once the tasks stop, without the message. Either way, the tasks keep their local cache entries, and the summary warns that they weren't uploaded because they were interrupted.
- Fast-fail doesn't cancel them. A task that succeeded before another task failed is still uploaded.

## Why interrupted tasks are not cached

A task that receives Ctrl-C might exit 0 after partial work (e.g., a build tool that flushes what it has so far). Caching this result would mean the next `vp run` replays incomplete output and skips the real execution. By never caching interrupted results, `vp run` guarantees that the next run starts fresh.
