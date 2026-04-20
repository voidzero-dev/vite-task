# chained_command_with____stops_at_first_failure

In a `&&`-chained command, a non-zero exit from the first sub-command should
short-circuit and skip the rest.

## `vt run pkg-a#chained`

first fails with exit code 3, second should not run

**Exit code:** 3

```
~/packages/pkg-a$ vtt exit 3
```
