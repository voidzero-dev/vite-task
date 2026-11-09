# errors

Errors go to stderr, and nothing is printed to stdout.

## `vt run --json build`

With a task, `--json` requires `--dry-run`.

**Exit code:** 2

```
error: with a task, '--json' is only supported with '--dry-run' for now

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

## `vt run --json --last-details`

`--last-details` takes no other argument.

**Exit code:** 2

```
error: the argument '--last-details' cannot be used with one or more of the other specified arguments

Usage: vt run [OPTIONS] [TASK_SPECIFIER] [ADDITIONAL_ARGS]...

For more information, try '--help'.
```

## `vt run --json -r`

Without a task, `--json` prints every package and task, and takes no other argument.

**Exit code:** 2

```
error: the argument '--json' cannot be used with one or more of the other specified arguments

Usage: vt run [OPTIONS] [TASK_SPECIFIER] [ADDITIONAL_ARGS]...

For more information, try '--help'.
```

## `vt run --dry-run --json`

Without a task, there is nothing to plan, as for `--dry-run`.

**Exit code:** 1

```
error: No task specifier provided for 'run' command
```

## `vt run --dry-run --json typo`

A task that isn't found is an error, not a task list.

**Exit code:** 1

```
error: Task "typo" not found
```

## `vt run --dry-run --json nested`

Nested runs aren't in the run plan yet.

**Exit code:** 1

```
error: `vt run --dry-run --json` doesn't support tasks that run `vt run` yet
```
