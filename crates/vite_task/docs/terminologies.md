# Terminologies

### Task-Related Names

```jsonc
// package.json
{
  "name": "app",
  "scripts": {
    "build": "echo build1 && echo build2"
  }
}
```

```jsonc
// vite-task.json
{
  "tasks": {
    "lint": "echo lint",
    "check": ["eslint .", "tsc --noEmit", "prettier --check ."]
  }
}
```

In the example above, `build`, `lint`, and `check` are **task group names**. A task group may define one task, or multiple tasks separated by `&&`.

In `tasks`, command-only task groups can be written as a string or as an array. Object form with `command` and options is also supported.

The three task groups generate these tasks:

- `app#build(subcommand 0)` (runs `echo build1`)
- `app#build` (runs `echo build2`)
- `app#lint` (runs `echo lint`)
- `app#check(subcommand 0)` (runs `eslint .`)
- `app#check(subcommand 1)` (runs `tsc --noEmit`)
- `app#check` (runs `prettier --check .`)

These are **task names**. They are for displaying and filtering.

The user could execute `vp run build` under the `app` package, or execute `vp run app#build` from anywhere. The parameter `build` and `app#build` after `vp run` are **task requests**. They are used to match against task names to determine what tasks to run.
