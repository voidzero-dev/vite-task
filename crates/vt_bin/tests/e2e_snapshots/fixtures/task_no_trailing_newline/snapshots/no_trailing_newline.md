# no_trailing_newline

Tests output handling when task has no trailing newline

## `vt run hello`

runs echo -n hello

```
$ echo -n foo ⊘ cache disabled
foo
$ echo bar ⊘ cache disabled
bar

---
vt run: (Run `vt run --last-details` for full details)
```
