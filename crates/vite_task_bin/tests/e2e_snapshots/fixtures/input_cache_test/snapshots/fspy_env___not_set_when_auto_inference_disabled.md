# fspy_env___not_set_when_auto_inference_disabled

When both input and output auto-inference are disabled, the task process should not see `FSPY` set.

## `vt run check-fspy-env-without-auto`

```
$ vtt print-env FSPY
(undefined)
```
