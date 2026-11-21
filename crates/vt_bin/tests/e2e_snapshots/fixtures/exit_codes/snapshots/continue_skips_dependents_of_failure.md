# continue_skips_dependents_of_failure

With `--continue`, a task that depends on a failed task still doesn't run.

## `vt run --continue -t check`

pkg-a fails, pkg-b is skipped

**Exit code:** 1

```
~/packages/pkg-a$ vtt exit 1
```
