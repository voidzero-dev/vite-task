# explicit_input_ignores_fspy_reads

When input auto is disabled (explicit globs only), unrelated reads
tracked by fspy must NOT become inferred inputs. Default auto output
still needs fspy for write tracking, but reads outside `input: ["src/**"]`
should be ignored.

## `vt run explicit-input-auto-output`

first run reads README.md (not in input globs) and captures the output

```
$ vtt print-file README.md
v1
```

## `vtt replace-file-content README.md v1 v2`

modify README.md — not an input, so it must not invalidate the cache

```
```

## `vt run explicit-input-auto-output`

cache hit: README.md not in input globs

```
$ vtt print-file README.md ◉ cache hit, replaying
v1

---
vt run: cache hit.
```
