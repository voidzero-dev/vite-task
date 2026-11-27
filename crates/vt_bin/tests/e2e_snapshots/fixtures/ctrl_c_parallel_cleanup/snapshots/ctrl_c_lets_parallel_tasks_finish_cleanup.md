# ctrl_c_lets_parallel_tasks_finish_cleanup

Ctrl+C with two parallel tasks: a exits 130 immediately, b takes a second to clean up. b shouldn't get killed when a exits.

## `vt run -r --parallel --no-cache dev`

**Exit code:** 130

**→ expect-milestone:** `ready`

```
~/packages/a$ vtt exit-on-ctrlc 0 130 ⊘ cache disabled
~/packages/b$ vtt exit-on-ctrlc 1000 ⊘ cache disabled
```

**→ expect-milestone:** `ready`

```
~/packages/a$ vtt exit-on-ctrlc 0 130 ⊘ cache disabled
~/packages/b$ vtt exit-on-ctrlc 1000 ⊘ cache disabled
```

**← write-key:** `ctrl-c`

```
~/packages/a$ vtt exit-on-ctrlc 0 130 ⊘ cache disabled
~/packages/b$ vtt exit-on-ctrlc 1000 ⊘ cache disabled
ctrl-c received
ctrl-c received

cleanup done

---
vt run: 1/2 successful. (Run `vt run --last-details` for full details)
```
