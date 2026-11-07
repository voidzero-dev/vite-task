# errors

Errors go to stderr, and nothing is printed to stdout.

## `vt run --json build`

With a task, `--json` requires `--dry-run`.

**Exit code:** 2

```
error: the argument '--json' requires '--dry-run' when a task is given

Usage: vt run [OPTIONS] [TASK_SPECIFIER] [ADDITIONAL_ARGS]...

For more information, try '--help'.
```

## `vt run --dry-run --json --log labeled build`

`--json` conflicts with `--log`.

**Exit code:** 2

```
error: the argument '--json' cannot be used with '--log <LOG>'

Usage: vt run --dry-run --json <TASK_SPECIFIER> <ADDITIONAL_ARGS>...

For more information, try '--help'.
```

## `vt run --dry-run --json typo`

A task that isn't found is an error, not a task list.

**Exit code:** 1

```
error: Task "typo" not found
```
