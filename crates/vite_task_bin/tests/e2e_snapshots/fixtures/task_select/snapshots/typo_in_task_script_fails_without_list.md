# typo_in_task_script_fails_without_list

A typo inside a task's own script (i.e. a nested `vp run` command) should
surface the real failure, not launch the task picker.

## `vt run run-typo-task`

**Exit code:** 1

```
Error: Failed to plan tasks from `vt run nonexistent-xyz` in task task-select-test#run-typo-task

Caused by:
    Task "nonexistent-xyz" not found
```
