# Defining Tasks

There are two ways to define tasks in Vite Task:

- **`scripts` in `package.json`**: Any script you define there can be run with `vite run <script-name>`, just like `npm run <script-name>`.
- **`vite-task.json`**: You can also define tasks in a dedicated `vite-task.json` file for more flexible configurations.

## `vite-task.json`

The `vite-task.json` file allows you to define tasks with more flexibility and options compared to `package.json` scripts. **It can be placed in in any packages alongside `package.json` to define tasks specific to that package.**

Here is an example of a `vite-task.json` file:

```json
{
  "tasks": {
    "build": {
      "command": "vite build",
      "cwd": "./packages/app",
      "env": {
        "NODE_ENV": "production"
      }
    },
    "test": {
      "command": "vite test",
      "cwd": "./packages/app"
    }
  }
}
```

Configurable fields for each task:

- `command` (string): The command to execute for the task.
- `cwd` (string, optional): The working directory to run the command in. Defaults to the directory containing the `vite-task.json` file.
- `dependsOn` (array of strings, optional): A list of other tasks that this task depends on. See [Task Orchestration](4%20-%20Task%20Orchestration) for details.
- `cache`, `envs`, `passthroughEnvs`, `inputs`, `outputs`: see [Configuring Cache](5%20-%20Configuring%20Cache.md) for details.

## Configuration Merging

### Merging with `package.json` Scripts

If your want to keep your existing `package.json` scripts, but also need more advanced configurations for some of them, you can define a task with the same name in `vite-task.json`. **Vite Task will use the configuration from `vite-task.json` and the command from `package.json`**.

For example, the following combination of `package.json` and `vite-task.json`:

```json
// package.json
{
    "scripts": {
        "build": "webpack",
    }
}
// vite-task.json
{
    "tasks": {
        "type-check": {
            "envs": ["NODE_ENV"]
        }
    }
}
```

is equivalent to:

```json
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

> If both script in `package.json` and `command` in `vite-task.json` are defined for the same name, Vite Task will report the conflict and abort.

### Merging with `defaults` in `vite-task.json`

You can provide default configurations by defining `defaults` in `vite-task.json` located in the root of the workspace. **Each configuration defined in `defaults` will be applied to all tasks in the workspace with the same name**, unless they explicitly override the configuration.

```json
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

> `defaults` field in non-root `vite-task.json` files are currently not allowed. Vite Task will report an error and abort if it sees one.
