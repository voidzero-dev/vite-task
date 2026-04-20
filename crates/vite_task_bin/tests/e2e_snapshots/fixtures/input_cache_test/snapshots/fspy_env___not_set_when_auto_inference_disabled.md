# fspy_env___not_set_when_auto_inference_disabled

When auto-inference is disabled (explicit globs only), the task process
should not see `FSPY` set.

## `vt run check-fspy-env-without-auto`

```
$ vtt print-env FSPY
(undefined)
```
