# Getting Started

Vite is designed to work out of the box with zero configuration. Simply run `vite run <script>` to execute any script defined in your `package.json` file.

For example, if you have the following script defined in your `package.json`:

```jsonc
// package.json
{
  "scripts": {
    "lint": "vite lint"
  }
}
```

The first time you run `vite run lint`, Vite Task will execute it just like `npm run lint`:

```
$ vite run lint

> vite lint

Found 0 warnings and 0 errors.
✓ Finished in 103ms on 3192 files with 88 rules using 10 threads.
```

If you run it again immediately, Vite Task will detect that nothing has changed and replay the cached output instantly without re-running the script:

```
$ vite run lint

> vite lint (cache hit, replaying)

Found 0 warnings and 0 errors.
✓ Finished in 103ms on 3192 files with 88 rules using 10 threads.
```

If you modify a source file and run it again, Vite Task will tell you why the cache was missed and re-execute the script:

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

## Running Tasks

### Single Package

Run a task in the current package:

```bash
vite run <task_name>
```

This executes the task defined in `vite-task.json` or `package.json` in the current directory.

### Monorepo

Target a specific package:

```bash
vite run <package_name>#<task_name>
```

Run a task across all packages:

```bash
vite run -r <task_name>
```

The `-r` (recursive) flag executes the task in all packages in topological order based on dependencies. This means if package A depends on package B, the task in B will run before the task in A.
