# unquoted_glob_preserves_extra_args_on_unix

Extra arguments passed to `vt run` are shell-escaped and appended after the package script. Shell operators inside one argument must remain literal after Unix shell expansion.

## `vt run unquoted-glob 'tail && vtt exit 31'`

```
$ vtt print packages/*/src ⊘ cache disabled
packages/a/src packages/b/src tail && vtt exit 31
```
