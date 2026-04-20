# cache_off_inherits_stdin

With caching off, stdin piped into `vp run` should flow through to the task
process.

## `vtt pipe-stdin from-stdin -- vt run read-stdin`

```
$ vtt read-stdin ⊘ cache disabled
from-stdin
```
