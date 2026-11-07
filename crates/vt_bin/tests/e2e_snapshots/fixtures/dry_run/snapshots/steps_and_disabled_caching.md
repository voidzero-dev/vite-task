# steps_and_disabled_caching

## `vt run --dry-run steps`

Each `&&` part is a step. The built-in `echo` prints nothing.

```
$ vtt print one
$ echo hello ⊘ cache disabled
$ vtt print two

---
vt run: 0/2 cache hit (0%), nothing was run.
```

## `vt run --dry-run uncached`

```
$ vtt print uncached ⊘ cache disabled
```
