# unknown_package_in_task_specifier_errors

A `package#task` specifier names its package explicitly, so a package that doesn't exist is an error rather than an empty `--filter`: the run exits 1, and the error goes to stderr with nothing on stdout.

## `vtt pipe-stdout -- vt run nonexistent#build`

**Exit code:** 1

```
error: Package 'nonexistent' not found in the workspace
--- stdout ---
```
