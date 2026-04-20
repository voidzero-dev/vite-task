# fspy_env___set_when_auto_inference_enabled

When auto-inference is enabled (input includes `{auto: true}`), the task
process should see `FSPY=1` in its environment.

## `vt run check-fspy-env-with-auto`

```
$ vtt print-env FSPY
1
```
