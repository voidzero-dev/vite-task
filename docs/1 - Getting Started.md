# Getting Started

Vite is designed to work out of the box with zero configuration. Simply run `vite run <script>` to execute any script defined in your `package.json` file.

For example, if you have the following script defined in your `package.json`:

```
// package.json
{
    "scripts": {
        "lint": "vite lint"
    }
}
```

The first time of `vite run build`, Vite Task will execute it just like `npm run build`:

```
$ vite run lint

> vite lint

Found 0 warnings and 0 errors.
✓ Finished in 103ms on 3192 files with 88 rules using 10 threads.
```

And if you run it again immediately, Vite Task will detect that nothing has changed and replay the cached ouput instantly without re-running the script:

```
$ vite run lint

> vite lint (cache hit, replaying)

Found 0 warnings and 0 errors.
✓ Finished in 103ms on 3192 files with 88 rules using 10 threads.
```

Then, if you modify some source files and run it again, Vite Task will re-execute the script to produce fresh output:

```
$ echo "debugger;" > src/index.js && vite run lint

> vite lint (cache miss, because the content of src/index.js changed)

  ⚠ eslint(no-debugger): `debugger` statement is not allowed
   ╭─[src/index.js:1:1]
 1 │ debugger;
   · ─────────
   ╰────
  help: Remove the debugger statement

Found 1 warning and 0 errors.
✓ Finished in 114ms on 3192 files with 88 rules using 10 threads.
```
