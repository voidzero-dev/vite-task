# transitive_typo_errors

`vt run -t <typo>` is not cwd-only, so a typo should error rather than fall back to the interactive selector.

## `vt run -t buid`

**Exit code:** 1

```
error: Task "buid" not found
```
