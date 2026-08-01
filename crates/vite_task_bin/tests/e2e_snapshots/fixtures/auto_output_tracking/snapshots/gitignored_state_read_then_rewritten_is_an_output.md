# gitignored_state_read_then_rewritten_is_an_output

A path a task reads and then rewrites is either a source it fixed up or state it manages, and access mechanics cannot tell those apart. Version control can: this file is gitignored, so it is derived state and belongs in the archive. Compare `input_read_write_not_cached`, where the same access pattern on a tracked source blocks caching instead.

## `vt run task`

```
~/packages/state-pkg$ vtt mkdir .cache

~/packages/state-pkg$ vtt cp src/data.txt .cache/state.txt

~/packages/state-pkg$ vtt replace-file-content .cache/state.txt e E

---
vt run: 0/3 cache hit (0%). (Run `vt run --last-details` for full details)
```

## `vt run task`

```
~/packages/state-pkg$ vtt mkdir .cache ◉ cache hit, replaying

~/packages/state-pkg$ vtt cp src/data.txt .cache/state.txt ◉ cache hit, replaying

~/packages/state-pkg$ vtt replace-file-content .cache/state.txt e E ◉ cache hit, replaying

---
vt run: 3/3 cache hit (100%). (Run `vt run --last-details` for full details)
```

## `vtt print-file .cache/state.txt`

```
statE
```
