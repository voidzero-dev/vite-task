# outside_git_repository_errors

Outside a git repository, `[<since>]` is an error, not an empty match that exits 0.

## `vt run --filter ...[HEAD~1] build`

**Exit code:** 1

```
error: Failed to find the packages changed since 'HEAD~1'
* <workspace> is not in a git repository: fatal: not a git repository (or any of the parent directories): .git
```
