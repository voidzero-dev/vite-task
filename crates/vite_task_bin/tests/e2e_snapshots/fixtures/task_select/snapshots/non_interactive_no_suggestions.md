# non_interactive_no_suggestions

In non-interactive mode, a typo with no close fuzzy matches should omit the
"did you mean" block entirely.

## `vtt pipe-stdin -- vt run zzzzz`

**Exit code:** 1

```
Task "zzzzz" not found.
```
