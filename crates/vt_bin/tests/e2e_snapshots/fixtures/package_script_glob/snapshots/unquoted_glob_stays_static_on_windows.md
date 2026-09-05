# unquoted_glob_stays_static_on_windows

Because `cmd.exe` does not perform pathname expansion, an unquoted pattern stays on the static execution path and is passed to the program literally.

## `vt run unquoted-glob`

```
$ vtt print packages/*/src ⊘ cache disabled
packages/*/src
```
