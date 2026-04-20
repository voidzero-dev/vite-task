# task_failure_fast_fails_remaining_tasks

A failing task under `-r` should fast-fail the run and skip any remaining packages.

## `vt run -r fail`

pkg-a fails, pkg-b is skipped

**Exit code:** 42

```
~/packages/pkg-a$ vtt exit 42
```
