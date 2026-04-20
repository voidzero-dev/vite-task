# non_interactive_recursive_typo_errors

A typoed task name combined with `-r` (not cwd-only) should error without
listing tasks — the `-r` signal rules out the interactive selector fallback.

## `vtt pipe-stdin -- vt run -r buid`

**Exit code:** 1

```
Error: Task "buid" not found
```
