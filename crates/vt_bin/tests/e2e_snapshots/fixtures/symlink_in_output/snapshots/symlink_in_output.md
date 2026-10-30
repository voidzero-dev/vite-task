# symlink_in_output

The task links `out/link` to `out/target.txt`, a file in the fixture, like a build tool linking a dependency into its output.

## `vt run build`

```
$ vtt symlink target.txt out/link
```

## `vtt rm out/link`

remove the link, as on a clean checkout

```
```

## `vt run build`

```
$ vtt symlink target.txt out/link ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt read-link out/link`

**Exit code:** 1

```
No such file or directory (os error 2)
```
