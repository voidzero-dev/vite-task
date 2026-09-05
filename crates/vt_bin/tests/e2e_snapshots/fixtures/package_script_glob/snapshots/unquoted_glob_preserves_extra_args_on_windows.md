# unquoted_glob_preserves_extra_args_on_windows

The static Windows execution path preserves extra argument boundaries without routing them through `cmd.exe`. Shell operators inside one argument must remain literal.

## `vt run unquoted-glob "tail && vtt exit 31"`

```
$ vtt print packages/*/src ⊘ cache disabled
packages/*/src tail && vtt exit 31
```
