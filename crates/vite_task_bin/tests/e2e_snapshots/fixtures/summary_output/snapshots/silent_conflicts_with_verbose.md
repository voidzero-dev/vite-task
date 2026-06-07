# silent_conflicts_with_verbose

`--silent` and `--verbose` are mutually exclusive; combining them is a usage error.

## `vt run --silent --verbose build`

**Exit code:** 2

```
error: the argument '--silent' cannot be used with '--verbose'

Usage: vt run --silent <TASK_SPECIFIER> <ADDITIONAL_ARGS>...

For more information, try '--help'.
```
