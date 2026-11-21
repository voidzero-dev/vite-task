# continue_survives_output_forwarding_failure

With `--continue`, a task that fails because its output can't be forwarded doesn't cancel the run. `vtt broken-stdout` gives `vt` a stdout whose writes all fail, so `noisy` fails when it prints. The scheduling is the same as in `continue_keeps_running_independent_tasks`: `noisy` waits at the barrier for one `independent-*` task, then prints and fails, and only the slot it frees can start the other `independent-*` task. The first step's output can't be seen. The second step, with a working stdout, replays `independent-1` from the cache (the two `independent-*` tasks run the same command, so they share a cache entry). Had the failure cancelled the run, the `independent-*` task waiting at the barrier would have been killed and the other never started, leaving nothing to replay.

## `vtt broken-stdout -- vt run --continue --concurrency-limit 2 forward-failure`

noisy fails to forward its output, the independent tasks pass

**Exit code:** 1

```
```

## `vt run independent-1`

setup and independent-1 replay from the cache

```
~/packages/pkg-c$ vtt exit 0 ◉ cache hit, replaying

~/packages/pkg-c$ vtt barrier ../../.barrier sync 3 ◉ cache hit, replaying

---
vt run: 2/2 cache hit (100%), 2/2 successful. (Run `vt run --last-details` for full details)
```
