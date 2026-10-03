# invalid_tag_is_not_a_cache_dir

A `CACHEDIR.TAG` with the wrong signature, or a directory named `CACHEDIR.TAG`, doesn't mark a cache directory.

## `vt run invalid-signature`

```
$ vtt replace-file-content invalid-signature/state.txt i !

---
vt run: cachedir-tag#invalid-signature not cached because it modified its inputs. (Run `vt run --last-details` for full details)
```

## `vt run tag-is-dir`

```
$ vtt replace-file-content tag-is-dir/state.txt i !

---
vt run: cachedir-tag#tag-is-dir not cached because it modified its inputs. (Run `vt run --last-details` for full details)
```
