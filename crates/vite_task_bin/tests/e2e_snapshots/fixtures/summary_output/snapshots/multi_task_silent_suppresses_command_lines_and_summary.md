# multi_task_silent_suppresses_command_lines_and_summary

`vt run -r --silent` should suppress every per-task command line and the multi-task summary, leaving only the tasks' own output.

## `vt run -r --silent build`

```
built-a
built-b
```
