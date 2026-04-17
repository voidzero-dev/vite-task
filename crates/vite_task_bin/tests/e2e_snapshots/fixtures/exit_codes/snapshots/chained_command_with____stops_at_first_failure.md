# chained_command_with____stops_at_first_failure

Tests exit code behavior for task failures

## `vt run pkg-a#chained`

first fails with exit code 3, second should not run

**Exit code:** 3

```
~/packages/pkg-a$ vtt exit 3
```
