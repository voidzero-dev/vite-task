# continue_skips_dependents_of_nested_run_failure

With `--continue`, a failure inside a nested `vt run` counts as its task failing, so that task's dependents don't run.

## `vt run --continue after-nested`

nested runs fail-3, which exits 3, so after-nested doesn't run

**Exit code:** 3

```
~/packages/pkg-c$ vtt exit 3
```
