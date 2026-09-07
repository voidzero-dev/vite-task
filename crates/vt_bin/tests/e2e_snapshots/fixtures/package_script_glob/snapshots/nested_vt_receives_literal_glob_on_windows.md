# nested_vt_receives_literal_glob_on_windows

On Windows, the static planner keeps nested `vt run` in process and forwards the pattern literally while preserving the preceding extra argument.

## `vt run nested-vt-glob`

```
$ vt run print-paths prefix packages/*/src ⊘ cache disabled
$ vtt print prefix packages/*/src ⊘ cache disabled
prefix packages/*/src
```
