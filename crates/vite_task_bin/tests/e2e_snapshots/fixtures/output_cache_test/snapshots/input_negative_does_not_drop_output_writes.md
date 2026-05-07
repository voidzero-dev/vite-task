# input_negative_does_not_drop_output_writes

Input negative globs must not drop matching writes from the output
archive. Here the user excludes `dist/**` from inferred inputs (so
rewriting dist/ won't mark inputs as modified), but default auto output
should still capture dist writes and restore them on a cache hit.

## `vt run input-neg-dist-auto-output`

first run writes dist/out.txt

```
$ vtt write-file dist/out.txt built
```

## `vtt rm dist/out.txt`

delete the output

```
```

## `vt run input-neg-dist-auto-output`

cache hit should restore dist/out.txt

```
$ vtt write-file dist/out.txt built ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt print-file dist/out.txt`

output was restored despite dist/** being a negative input glob

```
built
```
