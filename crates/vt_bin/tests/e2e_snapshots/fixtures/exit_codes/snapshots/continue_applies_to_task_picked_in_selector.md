# continue_applies_to_task_picked_in_selector

When the task name doesn't match and the task selector opens, `--continue` applies to the task picked there. `VP_RUN_CONCURRENCY_LIMIT=1` runs fail-3 and fail-5 one after the other, so the second only runs if the first failure doesn't cancel the run.

## `VP_RUN_CONCURRENCY_LIMIT=1 vt run --continue two-failure`

pick two-failures: fail-3 and fail-5 both run

**Exit code:** 1

**→ expect-milestone:** `task-select:two-failure:0`

```
Task "two-failure" not found.
Select a task (↑/↓, Enter to run, type to search): two-failure

  › two-failures vtt print should not run
```

**← write-key:** `enter`

```
Selected task: two-failures
~/packages/pkg-c$ vtt exit 3

~/packages/pkg-c$ vtt exit 5

---
vt run: 0/2 cache hit (0%), 0/2 successful. (Run `vt run --last-details` for full details)
```
