# windows_cached_pipe_handle_hang

A cached Windows leaf whose direct child exits while a descendant keeps stdout/stderr handles open should still observe the direct child exit and continue to the next `&&` item.

## `vt run test`

```
$ cmd /c start /b vtt barrier .hold stdio 1 --hang

$ vtt print after
after

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```
