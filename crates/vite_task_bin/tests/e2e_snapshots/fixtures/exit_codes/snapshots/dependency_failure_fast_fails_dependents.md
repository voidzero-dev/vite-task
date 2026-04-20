# dependency_failure_fast_fails_dependents

When a dependency fails under `-t`, dependents should be skipped rather than
run anyway.

## `vt run -t check`

pkg-a fails, pkg-b is skipped

**Exit code:** 1

```
~/packages/pkg-a$ vtt exit 1
```
