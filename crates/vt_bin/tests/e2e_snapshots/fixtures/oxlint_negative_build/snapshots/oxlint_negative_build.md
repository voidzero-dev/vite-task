# oxlint_negative_build

`oxlint -D no-debugger` lints the package, whose `.gitignore` lists `build/`, another task's output, so oxlint skips it. It lists the package root, which holds `build` once that task runs. The lint task has `"!build/**"` in its `input`.

## `vt run lint`

```
$ oxlint -D no-debugger
Found 0 warnings and 0 errors.
Finished in <duration> on 1 file with <n> rules using <n> threads.
```

## `vtt write-file build/out.js debugger;`

another task's `build` folder appears, holding code oxlint would reject if it didn't skip `build/`

```
```

## `vt run lint`

```
$ oxlint -D no-debugger ◉ cache hit, replaying
Found 0 warnings and 0 errors.
Finished in <duration> on 1 file with <n> rules using <n> threads.

---
vt run: cache hit.
```
