# stdin_is_always_null

In grouped mode, task stdin is always `/dev/null` regardless of the parent's
stdin — piping data in from outside must not reach the task.

## `vtt pipe-stdin from-stdin -- vt run --log=grouped read-stdin`

```
[grouped-stdio-test#read-stdin] $ vtt read-stdin ⊘ cache disabled
```
