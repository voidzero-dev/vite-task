# and_list_with_glob_stays_static_on_windows

Since the pathname pattern has no special meaning to `cmd.exe`, the existing static `&&` planning remains available on Windows while preserving ordering and literal arguments.

## `vt run and-glob`

```
$ vtt print before && vtt print packages/*/src ⊘ cache disabled
before
packages/*/src
```
