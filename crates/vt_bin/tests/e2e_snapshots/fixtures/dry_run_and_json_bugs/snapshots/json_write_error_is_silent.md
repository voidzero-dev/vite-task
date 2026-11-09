# json_write_error_is_silent

Bug: when the run plan can't be written to stdout, `vt run --dry-run --json` fails without saying why. `vt run --json` prints the error.

## `vtt closed-stdout -- vt run --json`

The task graph's write error goes to stderr.

**Exit code:** 1

```
error: <os error>
```

## `vtt closed-stdout -- vt run --dry-run --json build`

Expected: the same error. It exits 1 and prints nothing.

**Exit code:** 1

```
```
