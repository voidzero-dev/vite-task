# continue_keeps_running_independent_tasks

With `--continue`, a failing task doesn't cancel the run: a task running beside it finishes and is cached, new tasks still start, and a task that depends on the failure doesn't run.

`failing` and the two `independent-*` tasks share a barrier: `failing` waits for 2 participants, the others for 3. With a concurrency limit of 2, `failing` and `setup` start first. When `setup` exits, one `independent-*` task takes its slot and joins the barrier, so `failing` exits 42 while that task keeps waiting. Only the slot `failing` frees can start the third participant. If the failure cancelled the run, the waiting task would be killed; if it only stopped scheduling, the run would time out. `keep-going` depends on `failing`, so it doesn't run.

## `vt run --continue --concurrency-limit 2 keep-going`

failing exits 42, the independent tasks pass, keep-going doesn't run

**Exit code:** 42

```
~/packages/pkg-c$ vtt barrier ../../.barrier sync 2 --exit=42
~/packages/pkg-c$ vtt exit 0

~/packages/pkg-c$ vtt barrier ../../.barrier sync 3

~/packages/pkg-c$ vtt barrier ../../.barrier sync 3


---
vt run: 0/4 cache hit (0%), 3/4 successful. (Run `vt run --last-details` for full details)
```

## `vt run --continue --concurrency-limit 2 keep-going`

the independent tasks replay from the cache, failing runs again

**Exit code:** 42

```
~/packages/pkg-c$ vtt barrier ../../.barrier sync 2 --exit=42
~/packages/pkg-c$ vtt exit 0 ◉ cache hit, replaying

~/packages/pkg-c$ vtt barrier ../../.barrier sync 3 ◉ cache hit, replaying

~/packages/pkg-c$ vtt barrier ../../.barrier sync 3 ◉ cache hit, replaying


---
vt run: 3/4 cache hit (75%), 3/4 successful. (Run `vt run --last-details` for full details)
```
