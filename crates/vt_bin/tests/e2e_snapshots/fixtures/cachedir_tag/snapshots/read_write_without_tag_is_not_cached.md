# read_write_without_tag_is_not_cached

Without a `CACHEDIR.TAG`, the same task modifies its inputs and isn't cached.

## `vt run untagged`

```
$ vtt replace-file-content untagged/state.txt i !

---
vt run: cachedir-tag#untagged not cached because it modified its inputs. (Run `vt run --last-details` for full details)
```

## `vt run untagged`

```
$ vtt replace-file-content untagged/state.txt i !

---
vt run: cachedir-tag#untagged not cached because it modified its inputs. (Run `vt run --last-details` for full details)
```
