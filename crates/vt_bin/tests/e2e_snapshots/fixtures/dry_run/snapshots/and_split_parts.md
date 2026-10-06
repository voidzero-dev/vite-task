# and_split_parts

A later `&&` part of a task is unknown when an earlier part isn't a hit.

## `vt run both`

```
$ vtt print-file src.txt
source

$ vtt print-file test.txt
tests

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

## `vtt replace-file-content src.txt source changed`

modify an input of both parts

```
```

## `vt run --dry-run both`

the first part misses, the second is unknown

```
dry-run#both: $ vtt print-file src.txt → Cache miss: 'src.txt' modified
dry-run#both: $ vtt print-file test.txt → Unknown: runs after 'vtt print-file src.txt', which isn't a cache hit
```
