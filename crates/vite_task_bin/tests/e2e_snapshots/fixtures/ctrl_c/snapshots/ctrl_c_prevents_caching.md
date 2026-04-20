# ctrl_c_prevents_caching

A task interrupted by Ctrl+C must not populate the cache, even if it happens to
exit 0 — the following run should be a cache miss, not a hit.

## `vt run @ctrl-c/a#dev`

exits 0 but should not be cached

**→ expect-milestone:** `ready`

```
~/packages/a$ vtt exit-on-ctrlc
```

**← write-key:** `ctrl-c`

```
~/packages/a$ vtt exit-on-ctrlc
ctrl-c received
```

## `vt run @ctrl-c/a#dev`

should be cache miss, not hit

**→ expect-milestone:** `ready`

```
~/packages/a$ vtt exit-on-ctrlc
```

**← write-key:** `ctrl-c`

```
~/packages/a$ vtt exit-on-ctrlc
ctrl-c received
```
