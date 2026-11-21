# unknown_revision_errors

A revision git doesn't know is an error, not an empty match that exits 0.

## `git -c init.defaultBranch=main init -q`

```
```

## `git -c core.autocrlf=false add -A`

```
```

## `git -c user.name=test -c user.email=test@example.com -c commit.gpgsign=false commit -q --no-verify -m init`

```
```

## `vt run --filter ...[no-such-branch] build`

**Exit code:** 1

```
error: Failed to find the packages changed since 'no-such-branch'
* Unknown git revision 'no-such-branch'
```
