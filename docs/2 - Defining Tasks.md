# Defining Tasks

There are two ways to define tasks in Vite Task:

- `scripts` in `package.json`: Any script you define there can be run with `vite run <script-name>`, just like `npm run <script-name>`.
- `vite-task.json`: You can also define tasks in a dedicated `vite-task.json` file.

## `vite-task.json`

The `vite-task.json` file lets you define tasks with more options than `package.json` scripts. It should be placed in the root of any package alongside `package.json` to define tasks for that package.

Here is an example of a `vite-task.json` file:

```jsonc
{
  "tasks": {
    "build": {
      "command": "webpack",
      "envs": ["NODE_ENV"]
    },
    "test": {
      "command": "vite lint",
      "cwd": "./src"
    }
  }
}
```

Configurable fields for each task:

- `command` (string): The command to execute for the task.
- `cwd` (string, optional): The working directory to run the command in. Defaults to the directory containing the `vite-task.json` file.
- `dependsOn` (array of strings, optional): A list of other tasks that this task depends on. See [Task Orchestration](3%20-%20Task%20Orchestration.md) for details.
- `cache`, `envs`, `passthroughEnvs`, `inputs`, `outputs`: see [Configuring Cache](4%20-%20Configuring%20Cache.md) for details.

## Configuration Merging

### Merging with `package.json` Scripts

If you want to keep your existing `package.json` scripts but also need custom configurations for them, you can define a task with the same name in `vite-task.json`. **Vite Task will use the configuration from `vite-task.json` and the command from `package.json`**.

For example, the following combination of `package.json` and `vite-task.json`:

```jsonc
// package.json
{
    "scripts": {
        "build": "webpack",
    }
}
// vite-task.json
{
    "tasks": {
        "build": {
            "envs": ["NODE_ENV"]
        }
    }
}
```

is equivalent to:

```jsonc
// vite-task.json
{
  "tasks": {
    "build": {
      "command": "webpack",
      "envs": ["NODE_ENV"]
    }
  }
}
```

> If a script in `package.json` and a `command` in `vite-task.json` have the same name, Vite Task will report the conflict and abort.

### Merging with `defaults` in `vite-task.json`

You can provide default configurations by defining `defaults` in `vite-task.json` at the root of the workspace. **Each configuration defined in `defaults` will be applied to all tasks in the workspace with the same name**, unless a task explicitly overrides it.

```jsonc
// vite-task.json
{
  "defaults": {
    "build": {
      // Disable caching for all tasks named "build"
      "cache": false
    }
  }
}
```

> The `defaults` field is only allowed in the root `vite-task.json` file. Vite Task will report an error and abort if it finds one elsewhere.
