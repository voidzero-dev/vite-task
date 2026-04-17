# typo_in_task_script_fails_without_list

Non-interactive: list all tasks (piped stdin forces non-interactive mode)

## `vt run run-typo-task`

**Exit code:** 1

```
Error: Failed to plan tasks from `vt run nonexistent-xyz` in task task-select-test#run-typo-task

Caused by:
    Task "nonexistent-xyz" not found
```
