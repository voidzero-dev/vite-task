# single_task_failure_returns_task_exit_code

`vp run` should exit with the underlying task's exit code when a single task fails.

## `vt run pkg-a#fail`

exits with code 42

**Exit code:** 42

```
~/packages/pkg-a$ vtt exit 42
```
