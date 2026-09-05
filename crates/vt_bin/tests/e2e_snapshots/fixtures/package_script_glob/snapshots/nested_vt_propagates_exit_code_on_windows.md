# nested_vt_propagates_exit_code_on_windows

The static Windows path for an unquoted pathname pattern preserves a nested `vt run` failure exit code.

## `vt run nested-vt-glob-fails`

**Exit code:** 23

```
$ vt run fail-with-code packages/*/src ⊘ cache disabled
$ vtt exit 23 packages/*/src ⊘ cache disabled
```
