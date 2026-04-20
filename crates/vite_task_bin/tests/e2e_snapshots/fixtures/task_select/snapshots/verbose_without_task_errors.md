# verbose_without_task_errors

`vt run --verbose` with no task argument is not bare, so it should error
with "no task specifier provided" rather than opening the selector.

## `vt run --verbose`

**Exit code:** 1

```
Error: No task specifier provided for 'run' command
```
