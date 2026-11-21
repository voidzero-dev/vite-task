# continue_is_inherited_by_nested_run

`--continue` on the outer `vt run` also applies to a `vt run` in a task's script that doesn't pass it.

## `vt run --continue nested-plain`

the nested run inherits --continue: fail-3 and fail-5 both run

**Exit code:** 1

```
~/packages/pkg-c$ vtt exit 3

~/packages/pkg-c$ vtt exit 5

---
vt run: 0/2 cache hit (0%), 0/2 successful. (Run `vt run --last-details` for full details)
```
