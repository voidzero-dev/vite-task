# and_list_with_glob_uses_one_shell_on_unix

If any command in an `&&` list needs pathname expansion, the complete list is kept intact for the Unix shell so ordering and short-circuit semantics are preserved.

## `vt run and-glob`

```
$ vtt print before && vtt print packages/*/src ⊘ cache disabled
before
packages/a/src packages/b/src
```
