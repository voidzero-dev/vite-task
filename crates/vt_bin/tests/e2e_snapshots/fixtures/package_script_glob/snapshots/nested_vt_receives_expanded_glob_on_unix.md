# nested_vt_receives_expanded_glob_on_unix

An unquoted pathname pattern in a package script that invokes nested `vt run` is expanded by the shell before the new `vt` process starts. This verifies PATH lookup and preserves both preceding extra arguments and expanded paths.

## `vt run nested-vt-glob`

```
$ vt run print-paths prefix packages/*/src ⊘ cache disabled
$ vtt print prefix packages/a/src packages/b/src ⊘ cache disabled
prefix packages/a/src packages/b/src
```
