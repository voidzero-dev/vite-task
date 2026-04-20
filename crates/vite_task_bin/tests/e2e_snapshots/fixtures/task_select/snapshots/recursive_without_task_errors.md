# recursive_without_task_errors

`vt run -r` with no task argument is not treated as a bare run — it should
error instead of opening the selector.

## `vt run -r`

**Exit code:** 1

```
Error: No task specifier provided for 'run' command
```
