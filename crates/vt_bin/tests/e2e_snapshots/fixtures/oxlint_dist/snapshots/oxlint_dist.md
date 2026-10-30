# oxlint_dist

`oxlint -D no-debugger` lints the package, whose `.gitignore` lists `node_modules` but not `dist/`, like a GitHub Action that commits its built `dist/`. It lists the package root and each folder it doesn't ignore, and reads the files in them.

## `vt run lint`

```
$ oxlint -D no-debugger
Found 0 warnings and 0 errors.
Finished in <duration> on 1 file with 96 rules using <n> threads.
```

## `vtt write-file dist/bad.js debugger;`

a `dist` folder the task didn't write appears, holding code oxlint rejects

```
```

## `vt run lint`

```
$ oxlint -D no-debugger ◉ cache hit, replaying
Found 0 warnings and 0 errors.
Finished in <duration> on 1 file with 96 rules using <n> threads.

---
vt run: cache hit.
```
