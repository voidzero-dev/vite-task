# continue_chained_command_stops_at_first_failure

With `--continue`, a `&&`-chained command still stops at its first failing sub-command.

## `vt run --continue pkg-a#chained`

first fails with exit code 3, second should not run

**Exit code:** 3

```
~/packages/pkg-a$ vtt exit 3
```
