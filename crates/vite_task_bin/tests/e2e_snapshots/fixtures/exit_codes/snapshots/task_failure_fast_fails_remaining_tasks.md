# task_failure_fast_fails_remaining_tasks

Tests exit code behavior for task failures

## `vt run -r fail`

pkg-a fails, pkg-b is skipped

**Exit code:** 42

```
~/packages/pkg-a$ vtt exit 42
```
