# cache_on_gets_null_stdin

With caching on, task stdin should be replaced with `/dev/null` — piping data
in from outside must not reach the task.

## `vtt pipe-stdin from-stdin -- vt run read-stdin-cached`

```
$ vtt read-stdin
```
