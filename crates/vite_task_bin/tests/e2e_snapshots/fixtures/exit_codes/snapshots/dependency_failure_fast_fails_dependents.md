# dependency_failure_fast_fails_dependents

Tests exit code behavior for task failures

## `vt run -t check`

pkg-a fails, pkg-b is skipped

**Exit code:** 1

```
~/packages/pkg-a$ vtt exit 1
```
