# nested_vt_propagates_exit_code_on_unix

The shell fallback used for an unquoted pathname pattern preserves a nested `vt run` failure exit code after Unix shell expansion.

## `vt run nested-vt-glob-fails`

**Exit code:** 23

```
$ vt run fail-with-code packages/*/src ⊘ cache disabled
$ vtt exit 23 packages/a/src packages/b/src ⊘ cache disabled
```
